//! Startup-only privileges and explicit systemd readiness, no policy storage.
use anyhow::{ensure, Context, Result};
use std::{
    ffi::OsStr,
    os::unix::{ffi::OsStrExt, io::AsRawFd, net::UnixDatagram},
};

#[cfg(not(test))]
pub fn drop_startup_capabilities() -> Result<()> {
    // Linux capabilities are PER THREAD. Refuse to clear just one thread in a
    // multithread runtime. The production binary uses Tokio current_thread;
    // blocking workers are created only after this irreversible drop.
    ensure!(
        std::fs::read_dir("/proc/self/task")?.take(2).count() == 1,
        "Privilege drop requires single-thread startup"
    );
    #[repr(C)]
    struct Header {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let mut header = Header {
        version: 0x2008_0522,
        pid: 0,
    };
    let mut data = [Data::default(); 2];
    // SAFETY: prctl changes only this process/thread's privilege rules;
    // capset/capget use Linux capability ABI v3 and two initialized records.
    unsafe {
        ensure!(
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0,
            "NoNewPrivileges failed: {}",
            std::io::Error::last_os_error()
        );
        ensure!(
            libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_CLEAR_ALL,
                0,
                0,
                0
            ) == 0,
            "Ambient capability clear failed: {}",
            std::io::Error::last_os_error()
        );
        ensure!(
            libc::syscall(libc::SYS_capset, &header, data.as_ptr()) == 0,
            "Capability drop failed: {}",
            std::io::Error::last_os_error()
        );
        ensure!(
            libc::syscall(libc::SYS_capget, &mut header, data.as_mut_ptr()) == 0,
            "Capability verification failed"
        );
    }
    ensure!(
        data.iter()
            .all(|d| d.effective == 0 && d.permitted == 0 && d.inheritable == 0),
        "Capabilities remain after startup"
    );
    Ok(())
}

pub fn ready(required: bool) -> Result<()> {
    if !required {
        return Ok(());
    }
    let target =
        std::env::var_os("NOTIFY_SOCKET").context("--systemd-notify requires NOTIFY_SOCKET")?;
    send_ready(&target)
}
fn send_ready(target: &OsStr) -> Result<()> {
    let bytes = target.as_bytes();
    ensure!(
        !bytes.is_empty() && !bytes.contains(&0) && matches!(bytes[0], b'/' | b'@'),
        "Invalid NOTIFY_SOCKET"
    );
    // SAFETY: zeroed sockaddr_un is valid before filling family and address.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    ensure!(
        bytes.len() < address.sun_path.len(),
        "NOTIFY_SOCKET too long"
    );
    for (to, from) in address.sun_path.iter_mut().zip(bytes) {
        *to = *from as libc::c_char;
    }
    if bytes[0] == b'@' {
        address.sun_path[0] = 0;
    }
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path)
        + bytes.len()
        + usize::from(bytes[0] != b'@');
    let socket = UnixDatagram::unbound()?;
    socket.set_nonblocking(true)?;
    let message = b"READY=1\nSTATUS=Local ephemeral BAN ready; capacity=256; restart clears BANs";
    // SAFETY: checked AF_UNIX address/length, live socket and immutable message.
    let sent = unsafe {
        libc::sendto(
            socket.as_raw_fd(),
            message.as_ptr().cast(),
            message.len(),
            libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            (&address as *const libc::sockaddr_un).cast(),
            length as libc::socklen_t,
        )
    };
    ensure!(
        sent == message.len() as isize,
        "Readiness notification failed: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_is_explicit_bounded_and_supports_abstract_socket() {
        assert!(ready(false).is_ok());
        for bad in ["", "relative", "@bad\0name"] {
            assert!(send_ready(OsStr::new(bad)).is_err());
        }
        use std::os::linux::net::SocketAddrExt;
        let mut token = [0; 12];
        use ring::rand::SecureRandom;
        ring::rand::SystemRandom::new().fill(&mut token).unwrap();
        let name = format!("veil-notify-{:x?}", token);
        let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let receiver = UnixDatagram::bind_addr(&address).unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        send_ready(OsStr::new(&format!("@{name}"))).unwrap();
        let mut buffer = [0; 256];
        let count = receiver.recv(&mut buffer).unwrap();
        assert!(std::str::from_utf8(&buffer[..count])
            .unwrap()
            .starts_with("READY=1\n"));
    }
}
