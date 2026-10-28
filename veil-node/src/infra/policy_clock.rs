//! Bound signed UTC deadlines, then enforce them using the suspend-aware boot clock.
/// Non-Linux daemons can participate in consensus, but must not issue leases
/// using the deterministic simulator clock. Default lease_time fails closed.
#[cfg(not(target_os = "linux"))]
pub struct UnavailableLeaseClock(pub std::time::Instant);

#[cfg(not(target_os = "linux"))]
impl crate::usecase::ports::PolicyClock for UnavailableLeaseClock {
    fn boot_id(&self) -> &str {
        "unavailable"
    }
    fn now_ns(&self) -> std::io::Result<u64> {
        u64::try_from(self.0.elapsed().as_nanos()).map_err(std::io::Error::other)
    }
}

#[cfg(target_os = "linux")]
pub struct LinuxBootClock {
    boot_id: String,
    max_error_ns: u64,
}

#[cfg(target_os = "linux")]
impl LinuxBootClock {
    pub fn new() -> std::io::Result<Self> {
        Self::with_max_error_ms(1_000)
    }

    pub fn with_max_error_ms(max_error_ms: u64) -> std::io::Result<Self> {
        let max_error_ns = max_error_ms
            .checked_mul(1_000_000)
            .filter(|&n| n > 0)
            .ok_or_else(|| std::io::Error::other("Invalid clock error budget"))?;
        // BOOTTIME in a time namespace may not match the kernel BPF helper.
        match std::fs::read_to_string("/proc/self/timens_offsets") {
            Ok(offsets) => {
                let boot = offsets
                    .lines()
                    .find(|l| l.split_whitespace().next() == Some("boottime"))
                    .ok_or_else(|| std::io::Error::other("Missing boottime namespace offset"))?;
                if boot.split_whitespace().skip(1).collect::<Vec<_>>() != ["0", "0"] {
                    return Err(std::io::Error::other(
                        "Nonzero boottime namespace offset is unsupported",
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
        if boot_id.trim().is_empty() {
            return Err(std::io::Error::other("Empty Linux boot ID"));
        }
        Ok(Self {
            boot_id,
            max_error_ns,
        })
    }
}

#[cfg(target_os = "linux")]
impl crate::usecase::ports::PolicyClock for LinuxBootClock {
    fn boot_id(&self) -> &str {
        &self.boot_id
    }

    fn now_ns(&self) -> std::io::Result<u64> {
        read_clock(libc::CLOCK_BOOTTIME)
    }

    fn lease_time(&self) -> std::io::Result<crate::usecase::ports::LeaseTime> {
        let boot_ns = self.now_ns()?;
        // SAFETY: timex is a C POD, initialized with modes=0. This call only
        // queries clock state; it never adjusts the host clock or NTP settings.
        let mut tx: libc::timex = unsafe { std::mem::zeroed() };
        let status = unsafe { libc::adjtimex(&mut tx) };
        if status == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let uncertainty_ns =
            checked_uncertainty(status, tx.status, tx.maxerror, self.max_error_ns)?;
        let unix_ns = read_clock(libc::CLOCK_REALTIME)?;
        Ok(crate::usecase::ports::LeaseTime {
            boot_ns,
            unix_ns,
            uncertainty_ns,
            // Operational rate assumption: at most 0.1% slow, including clock
            // discipline and oscillator error. The lease is shortened accordingly.
            max_slow_rate_ppm: 1_000,
        })
    }
}

#[cfg(target_os = "linux")]
fn checked_uncertainty(
    status: i32,
    flags: i32,
    maxerror_us: libc::c_long,
    budget_ns: u64,
) -> std::io::Result<u64> {
    if status != libc::TIME_OK
        || flags & (libc::STA_UNSYNC | libc::STA_CLOCKERR | libc::STA_INS | libc::STA_DEL) != 0
    {
        return Err(std::io::Error::other(format!(
            "Clock is unsynchronized, faulty, or in a leap transition: status={status} flags={flags:#x}"
        )));
    }
    let uncertainty = u64::try_from(maxerror_us)
        .ok()
        .and_then(|us| us.checked_mul(1_000))
        .ok_or_else(|| std::io::Error::other("Invalid kernel clock error bound"))?;
    if uncertainty > budget_ns {
        return Err(std::io::Error::other(format!(
            "Kernel clock error exceeds configured budget: maxerror_us={maxerror_us} budget_ns={budget_ns}"
        )));
    }
    Ok(uncertainty)
}

#[cfg(target_os = "linux")]
fn read_clock(clock_id: libc::clockid_t) -> std::io::Result<u64> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts is writable and correctly aligned; callers use valid Linux clock IDs.
    if unsafe { libc::clock_gettime(clock_id, &mut ts) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let seconds = u64::try_from(ts.tv_sec).map_err(std::io::Error::other)?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add(ts.tv_nsec as u64))
        .ok_or_else(|| std::io::Error::other("Boot clock overflow"))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn clock_quality_is_required_without_a_success_fallback() {
        assert_eq!(
            checked_uncertainty(libc::TIME_OK, 0, 50, 50_000).unwrap(),
            50_000
        );
        assert!(checked_uncertainty(libc::TIME_ERROR, 0, 0, 50_000).is_err());
        assert!(checked_uncertainty(libc::TIME_OK, libc::STA_UNSYNC, 0, 50_000).is_err());
        assert!(checked_uncertainty(libc::TIME_OK, libc::STA_INS, 0, 50_000).is_err());
        assert!(checked_uncertainty(libc::TIME_OK, 0, -1, 50_000).is_err());
        assert!(checked_uncertainty(libc::TIME_OK, 0, 51, 50_000).is_err());
    }

    #[test]
    fn synchronized_clock_can_exceed_budget_between_ntp_updates() {
        // Linux's 500 us/s maxerror growth exceeds the unchanged one-second
        // budget over timesyncd's default 2048-second maximum poll interval.
        let budget = 1_000_000_000;
        assert_eq!(
            checked_uncertainty(libc::TIME_OK, 0, 1_000_000, budget).unwrap(),
            budget
        );
        let error = checked_uncertainty(libc::TIME_OK, 0, 2048 * 500, budget).unwrap_err();
        assert!(error
            .to_string()
            .contains("maxerror_us=1024000 budget_ns=1000000000"));
        // A subsequent successful synchronization may make the clock usable
        // again. There is no sticky failure and no fallback to estimated error.
        assert_eq!(
            checked_uncertainty(libc::TIME_OK, 0, 128_000, budget).unwrap(),
            128_000_000
        );
    }
}
