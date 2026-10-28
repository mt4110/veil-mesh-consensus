//! Owner-only, fixed-size administration. One admitted mutation may wait for
//! sweep I/O; competing mutations are rejected without a waiting queue.
use super::{
    attach_guard::AttachMode,
    kernel::{self, Kernel},
    sockets::{validate_dir, ControlSockets, EMERGENCY, NORMAL},
    Backend, Engine, CAPACITY, STEP_NS,
};
use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand};
use ring::rand::{SecureRandom, SystemRandom};
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Mutex, Semaphore},
    task::JoinSet,
};

const FRAME: usize = 40;
const IO_LIMIT: Duration = Duration::from_secs(2);
const RESPONSE_LIMIT: usize = 4096;
const LIST_RESPONSE_LIMIT: usize = 65_536;
#[derive(Parser)]
#[command(about = "Volatile local IPv4/TCP BANs: restart clears every BAN")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Read-only runtime metadata. Does not recover, delete or probe sockets.
    InspectRuntime {
        #[arg(long)]
        control_dir: PathBuf,
    },
    Serve {
        /// Require and send systemd READY only after initialization.
        #[arg(long)]
        systemd_notify: bool,
        #[arg(long)]
        iface: String,
        /// Fixed startup mode; no fallback or automatic NIC configuration.
        #[arg(long, value_enum, default_value = "default")]
        xdp_mode: AttachMode,
        #[arg(long)]
        ebpf_object: String,
        #[arg(long)]
        control_dir: PathBuf,
        #[arg(long)]
        destination: Ipv4Addr,
        #[arg(long)]
        port: u16,
        #[arg(long)]
        protected_ip: Vec<Ipv4Addr>,
    },
    Status {
        #[arg(long)]
        control_dir: PathBuf,
        #[arg(long)]
        ip: Option<Ipv4Addr>,
    },
    /// Snapshot of at most 256 tracked BANs, including expired/pending entries.
    List {
        #[arg(long)]
        control_dir: PathBuf,
    },
    /// Read-only preview; prints a command with frozen instance/revision.
    PreviewBan {
        #[arg(long)]
        control_dir: PathBuf,
        #[arg(long)]
        ip: Ipv4Addr,
        #[arg(long)]
        ttl_secs: u32,
    },
    /// Read-only preview; never automatically retries or applies a mutation.
    PreviewUnban {
        #[arg(long)]
        control_dir: PathBuf,
        #[arg(long)]
        ip: Ipv4Addr,
    },
    Ban {
        #[arg(long)]
        control_dir: PathBuf,
        #[arg(long)]
        ip: Ipv4Addr,
        #[arg(long)]
        ttl_secs: u32,
        #[arg(long)]
        instance: String,
        #[arg(long)]
        expected_revision: u64,
    },
    Unban {
        #[arg(long)]
        control_dir: PathBuf,
        #[arg(long)]
        ip: Ipv4Addr,
        #[arg(long)]
        instance: String,
        #[arg(long)]
        expected_revision: u64,
    },
    Bypass {
        #[arg(long)]
        control_dir: PathBuf,
        #[arg(long, value_parser=["on","off"])]
        mode: String,
        #[arg(long)]
        instance: String,
        #[arg(long)]
        expected_revision: u64,
    },
}
#[derive(Clone, Copy)]
struct Request {
    op: u8,
    instance: [u8; 16],
    expected: u64,
    ip: u32,
    ttl: u32,
}
impl Request {
    fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == FRAME && &bytes[..4] == b"VLB1" && bytes[5..8] == [0; 3],
            "INVALID_FRAME"
        );
        let r = Self {
            op: bytes[4],
            instance: bytes[8..24].try_into()?,
            expected: u64::from_be_bytes(bytes[24..32].try_into()?),
            ip: u32::from_be_bytes(bytes[32..36].try_into()?),
            ttl: u32::from_be_bytes(bytes[36..40].try_into()?),
        };
        ensure!(
            match r.op {
                0 => r.instance == [0; 16] && r.expected == 0 && r.ttl == 0,
                1 => true,
                2 => r.ttl == 0,
                3 | 4 => r.ip == 0 && r.ttl == 0,
                5 => r.instance == [0; 16] && r.expected == 0 && r.ip == 0 && r.ttl == 0,
                6 => r.instance == [0; 16] && r.expected == 0,
                7 => r.instance == [0; 16] && r.expected == 0 && r.ttl == 0,
                _ => false,
            },
            "INVALID_FRAME"
        );
        Ok(r)
    }
    fn encode(self) -> [u8; FRAME] {
        let mut b = [0; FRAME];
        b[..4].copy_from_slice(b"VLB1");
        b[4] = self.op;
        b[8..24].copy_from_slice(&self.instance);
        b[24..32].copy_from_slice(&self.expected.to_be_bytes());
        b[32..36].copy_from_slice(&self.ip.to_be_bytes());
        b[36..40].copy_from_slice(&self.ttl.to_be_bytes());
        b
    }
}
fn decode_instance(s: &str) -> Result<[u8; 16]> {
    ensure!(
        s.len() == 32 && s.is_ascii(),
        "Instance must be 32 hex characters"
    );
    let mut b = [0; 16];
    for (i, x) in b.iter_mut().enumerate() {
        *x = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)?;
    }
    Ok(b)
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect()
}
fn err(code: &str) -> String {
    format!("ERR\ncode={code}\n")
}
struct Shared {
    engine: Arc<Mutex<Engine>>,
    mutations: Arc<Semaphore>,
    backend: Arc<dyn Backend>,
    instance: [u8; 16],
    control: std::sync::Mutex<()>,
    control_revision: AtomicU64,
    bypass: AtomicBool,
    destination: Ipv4Addr,
    port: u16,
}
impl Shared {
    fn inspect_policy(&self, r: Request) -> String {
        let Ok(e) = self.engine.try_lock() else {
            return err("BUSY");
        };
        let now = match kernel::boot_now() {
            Ok(now) => now,
            Err(_) => return err("CLOCK_UNAVAILABLE"),
        };
        let validation = match r.op {
            6 => e.preview_ban(r.ip, r.ttl, now),
            7 => e.preview_unban(r.ip, now),
            _ => Ok(()),
        };
        if let Err(code) = validation {
            return err(code);
        }
        let revision = e.revision();
        let entries = e.entries();
        drop(e); // No map syscall, formatting or network I/O under the policy lock.
        let mut s = format!(
            "OK\nmode=local-ephemeral\nsource=userspace-mirror\npersistence=none\ninstance={}\nrevision={revision}\nsnapshot_boot_ns={now}\ndestination={}\nprotocol=tcp\nport={}\nbypass_observed={}\ncapacity={CAPACITY}\n",
            hex(&self.instance), self.destination, self.port, self.bypass.load(Ordering::SeqCst)
        );
        if r.op == 5 {
            let count = entries.iter().flatten().count();
            s += &format!("entries={count}\n");
            for entry in entries.iter().flatten() {
                let remaining = entry.expires_at_boot_ns.saturating_sub(now);
                let seconds = remaining / 1_000_000_000 + u64::from(remaining % 1_000_000_000 != 0);
                let state = if remaining == 0 {
                    "expired"
                } else if entry.confirmed {
                    "active"
                } else {
                    "pending"
                };
                s += &format!("entry={},state={state},confirmed={},expires_at_boot_ns={},remaining_secs_ceil={seconds}\n",
                    entry.ip, entry.confirmed, entry.expires_at_boot_ns);
            }
        } else {
            let ip = Ipv4Addr::from(r.ip);
            let current = entries.iter().flatten().find(|entry| entry.ip == ip);
            let effect = match (r.op, current.is_some()) {
                (6, true) => "replace-existing-lease",
                (6, false) => "new-lease",
                _ => "remove-lease-if-present",
            };
            s += &format!("preview=true\noperation={}\nip={ip}\nttl_secs={}\neffect={effect}\ntracked={}\ncurrent_expires_at_boot_ns={}\nchecks=advisory-not-reserved\n",
                if r.op == 6 { "ban" } else { "unban" }, r.ttl, current.is_some(), current.map(|e| e.expires_at_boot_ns).unwrap_or(0));
        }
        s
    }
    fn status(&self, ip: u32) -> String {
        let mut s=format!("OK\nmode=local-ephemeral\npersistence=none\ninstance={}\ncontrol_revision={}\nbypass={}\ncapacity={CAPACITY}\n",hex(&self.instance),self.control_revision.load(Ordering::SeqCst),self.bypass.load(Ordering::SeqCst));
        if let Ok(e) = self.engine.try_lock() {
            match kernel::boot_now() {
                Ok(now)=>s+=&format!("revision={}\noccupied={}\nactive={}\nexpired={}\npending={}\nevent_count={}\ncleanup_steps={}\nmax_examined={}\nmax_attempted={}\nlast_examined={}\nlast_attempted={}\nlast_removed={}\npolicy_mirror={}\nexpires_at_boot_ns={}\n",e.revision(),e.occupied(),e.active(now),e.occupied()-e.active(now),e.pending(),e.event_count(),e.steps,e.max_examined,e.max_attempted,e.last_sweep.examined,e.last_sweep.attempted,e.last_sweep.removed,e.verdict(ip,now),e.deadline(ip).unwrap_or(0)),
                Err(_)=>s+="clock=unavailable\n",
            }
        } else {
            s += "policy_busy=true\n";
        }
        match self.backend.counters() {
            Ok((pass, drop)) => s += &format!("kernel_pass={pass}\nkernel_drop={drop}\n"),
            Err(_) => s += "counters=unavailable\n",
        };
        s
    }
    fn bypass(&self, r: Request) -> String {
        let Ok(_guard) = self.control.try_lock() else {
            return err("BUSY");
        };
        let rev = self.control_revision.load(Ordering::SeqCst);
        if rev != r.expected {
            return err("REVISION_CONFLICT");
        }
        let Some(next) = rev.checked_add(1) else {
            return err("REVISION_EXHAUSTED");
        };
        self.control_revision.store(next, Ordering::SeqCst);
        let enabled = r.op == 3;
        if self.backend.bypass(enabled).is_err() {
            return err("MAP_CONTROL_FAILED");
        }
        self.bypass.store(enabled, Ordering::SeqCst);
        format!("OK\ncontrol_revision={next}\nbypass={enabled}\n")
    }
}
async fn read_request(stream: &mut UnixStream) -> Result<Request> {
    let mut bytes = [0; FRAME];
    stream.read_exact(&mut bytes).await?;
    let mut extra = [0; 1];
    ensure!(stream.read(&mut extra).await? == 0, "EXTRA_BYTES");
    Request::parse(&bytes)
}
async fn handle(
    mut stream: UnixStream,
    shared: Arc<Shared>,
    emergency: bool,
    permit: tokio::sync::OwnedSemaphorePermit,
) {
    // Keep the admission slot through response writing as well as any detached syscall.
    let permit = Arc::new(permit);
    let _handler_permit = permit.clone();
    // The directory is owner-only; SO_PEERCRED is an additional check.
    if stream.peer_cred().map(|c| c.uid()).ok() != Some(unsafe { libc::geteuid() }) {
        return;
    }
    let r = match tokio::time::timeout(IO_LIMIT, read_request(&mut stream)).await {
        Ok(Ok(r)) => r,
        _ => {
            let _ =
                tokio::time::timeout(IO_LIMIT, stream.write_all(err("INVALID_FRAME").as_bytes()))
                    .await;
            return;
        }
    };
    if (emergency && !matches!(r.op, 0 | 3 | 4)) || (!emergency && matches!(r.op, 3 | 4)) {
        let _ = tokio::time::timeout(IO_LIMIT, stream.write_all(err("WRONG_ENDPOINT").as_bytes()))
            .await;
        return;
    }
    if matches!(r.op, 1..=4) && r.instance != shared.instance {
        let _ = tokio::time::timeout(
            IO_LIMIT,
            stream.write_all(err("INSTANCE_CONFLICT").as_bytes()),
        )
        .await;
        return;
    }
    let job = if matches!(r.op, 1 | 2) {
        match shared.mutations.clone().try_acquire_owned() {
            Ok(admission) => tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let _admission = admission;
                // Only one mutation may be admitted, including a timed-out
                // worker. Wait for a running sweep inside the existing job
                // deadline, without reserving the async reactor or admitting
                // a queue of competing mutations.
                let mut e = shared.engine.clone().blocking_lock_owned();
                let now = match kernel::boot_now() {
                    Ok(n) => n,
                    Err(_) => return err("CLOCK_UNAVAILABLE"),
                };
                let result = if r.op == 1 {
                    e.ban(r.ip, r.ttl, r.expected, now)
                } else {
                    e.unban(r.ip, r.expected, now)
                };
                match result {
                    Ok(()) => format!("OK\nrevision={}\nresult=APPLIED\n", e.revision()),
                    Err(code) => format!("ERR\ncode={code}\nrevision={}\n", e.revision()),
                }
            }),
            Err(_) => {
                let _ =
                    tokio::time::timeout(IO_LIMIT, stream.write_all(err("BUSY").as_bytes())).await;
                return;
            }
        }
    } else {
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            match r.op {
                0 => shared.status(r.ip),
                3 | 4 => shared.bypass(r),
                5..=7 => shared.inspect_policy(r),
                _ => err("INVALID_FRAME"),
            }
        })
    };
    // A timed-out caller does not cancel a syscall or release its worker permit.
    let response = match tokio::time::timeout(IO_LIMIT, job).await {
        Ok(Ok(s)) => s,
        Ok(Err(_)) => err("WORKER_FAILED"),
        Err(_) => err("OUTCOME_UNKNOWN_CHECK_STATUS"),
    };
    let _ = tokio::time::timeout(IO_LIMIT, stream.write_all(response.as_bytes())).await;
}
async fn accept_loop(
    listener: UnixListener,
    shared: Arc<Shared>,
    limit: usize,
    emergency: bool,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let permits = Arc::new(Semaphore::new(limit));
    let mut jobs = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _=stop.changed()=>break,
            result=jobs.join_next(),if !jobs.is_empty()=>{if let Some(Err(e))=result {eprintln!("Control task failed: {e}");}},
            accepted=listener.accept()=>{
                let (stream,_)=accepted?;
                if let Ok(permit)=permits.clone().try_acquire_owned() {jobs.spawn(handle(stream,shared.clone(),emergency,permit));}
                // Excess accepted connections are closed immediately, with no waiting queue.
            }
        }
    }
    while jobs.join_next().await.is_some() {}
    // Also wait for timed-out blocking jobs, which retain their permits.
    let _all = permits.acquire_many(limit as u32).await?;
    Ok(())
}
async fn sweep_loop(
    shared: Arc<Shared>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let mut tick = tokio::time::interval(Duration::from_nanos(STEP_NS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {biased;_=stop.changed()=>return Ok(()),_=tick.tick()=>{}}
        let engine = shared.engine.clone();
        // Do not reserve the engine while this sweep waits in the blocking
        // pool. With current_thread that would reject unrelated mutations as
        // BUSY before the bounded sweep has even started doing work.
        tokio::task::spawn_blocking(move || {
            if let Ok(mut engine) = engine.try_lock_owned() {
                if let Ok(now) = kernel::boot_now() {
                    let _ = engine.sweep(now);
                }
            }
        })
        .await?;
    }
}
async fn serve(
    iface: String,
    object: String,
    dir: PathBuf,
    destination: Ipv4Addr,
    port: u16,
    mut protected: Vec<Ipv4Addr>,
    systemd_notify: bool,
    xdp_mode: AttachMode,
) -> Result<()> {
    validate_dir(&dir)?;
    kernel::check_clock_namespace()?;
    protected.push(destination);
    ensure!(
        protected.len() <= 256,
        "At most 255 additional protected IPs"
    );
    let mut instance = [0; 16];
    SystemRandom::new()
        .fill(&mut instance)
        .map_err(|_| anyhow::anyhow!("Random instance failed"))?;
    let ControlSockets {
        normal,
        emergency,
        owners: _owners,
    } = ControlSockets::bind(&dir).context("Runtime initialization refused; use inspect-runtime and the manual recovery guide; preserve unknown paths")?;
    let kernel = Kernel::attach(&object, &iface, destination, port, xdp_mode)?;
    let actual_mode = kernel.actual_mode;
    let program_id = kernel.program_id;
    let backend: Arc<dyn Backend> = Arc::new(kernel);
    #[cfg(test)]
    super::sockets::test_checkpoint("xdp_attached");
    #[cfg(not(test))]
    super::service::drop_startup_capabilities()?;
    let engine = Engine::new(backend.clone(), &protected).map_err(anyhow::Error::msg)?;
    let shared = Arc::new(Shared {
        engine: Arc::new(Mutex::new(engine)),
        mutations: Arc::new(Semaphore::new(1)),
        backend,
        instance,
        control: std::sync::Mutex::new(()),
        control_revision: AtomicU64::new(0),
        bypass: AtomicBool::new(false),
        destination,
        port,
    });
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let (stop, rx) = tokio::sync::watch::channel(false);
    let mut tasks = JoinSet::new();
    tasks.spawn(accept_loop(normal, shared.clone(), 8, false, rx.clone()));
    tasks.spawn(accept_loop(emergency, shared.clone(), 1, true, rx.clone()));
    tasks.spawn(sweep_loop(shared.clone(), rx));
    println!(
        "READY mode=local-ephemeral instance={} capacity=256 restart=clears-bans xdp_requested={} xdp_actual={} program_id={} offload_guard=active-wanted-off",
        hex(&instance), xdp_mode.name(), actual_mode, program_id
    );
    let failure = match super::service::ready(systemd_notify) {
        Err(e) => Some(e),
        Ok(()) => tokio::select! {
            _ = terminate.recv() => None,
            _ = interrupt.recv() => None,
            task=tasks.join_next()=>Some(anyhow::anyhow!("Runtime task ended: {task:?}")),
        },
    };
    let _ = stop.send(true);
    while tasks.join_next().await.is_some() {}
    // All worker references are gone before closing the FD-owned XDP link.
    drop(shared);
    if let Some(e) = failure {
        return Err(e);
    }
    Ok(())
}
fn shell_arg(value: &str) -> Result<String> {
    ensure!(
        !value.chars().any(char::is_control),
        "Control characters cannot be rendered safely"
    );
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

fn preview_command(executable: &Path, dir: &Path, r: Request, response: &str) -> Result<String> {
    ensure!(matches!(r.op, 6 | 7), "Not a preview");
    let mut lines = response.lines();
    ensure!(lines.next() == Some("OK"), "Preview refused");
    let mut fields = std::collections::BTreeMap::new();
    for line in lines {
        let (key, value) = line.split_once('=').context("Invalid preview response")?;
        ensure!(
            fields.insert(key, value).is_none(),
            "Duplicate preview field"
        );
    }
    let field = |key| fields.get(key).copied().context("Missing preview field");
    let operation = if r.op == 6 { "ban" } else { "unban" };
    ensure!(
        field("preview")? == "true" && field("operation")? == operation,
        "Unexpected preview operation"
    );
    ensure!(
        field("ip")?.parse::<Ipv4Addr>()? == Ipv4Addr::from(r.ip)
            && field("ttl_secs")?.parse::<u32>()? == r.ttl,
        "Unexpected preview target/TTL"
    );
    let instance = hex(&decode_instance(field("instance")?)?);
    let revision = field("revision")?.parse::<u64>()?;
    let mut args = vec![
        executable
            .to_str()
            .context("Executable path must be UTF-8")?
            .to_owned(),
        operation.to_owned(),
        "--control-dir".into(),
        dir.to_str()
            .context("Control directory must be UTF-8")?
            .to_owned(),
        "--ip".into(),
        Ipv4Addr::from(r.ip).to_string(),
    ];
    if r.op == 6 {
        args.extend(["--ttl-secs".into(), r.ttl.to_string()]);
    }
    args.extend([
        "--instance".into(),
        instance,
        "--expected-revision".into(),
        revision.to_string(),
    ]);
    args.iter()
        .map(|v| shell_arg(v))
        .collect::<Result<Vec<_>>>()
        .map(|v| v.join(" "))
}

async fn exchange(dir: &Path, r: Request) -> Result<String> {
    validate_dir(dir)?;
    let limit = if r.op == 5 {
        LIST_RESPONSE_LIMIT
    } else {
        RESPONSE_LIMIT
    };
    let response = tokio::time::timeout(IO_LIMIT, async {
        let mut stream = UnixStream::connect(dir.join(if matches!(r.op, 3 | 4) {
            EMERGENCY
        } else {
            NORMAL
        }))
        .await?;
        stream.write_all(&r.encode()).await?;
        stream.shutdown().await?;
        let mut bytes = Vec::with_capacity(1024);
        stream
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        ensure!(bytes.len() <= limit, "Oversized response");
        Ok::<_, anyhow::Error>(String::from_utf8(bytes)?)
    })
    .await
    .context("Outcome unknown: inspect status; do not automatically retry a mutation")??;
    Ok(response)
}

async fn client(dir: PathBuf, r: Request) -> Result<()> {
    let response = exchange(&dir, r).await?;
    print!("{response}");
    if !response.starts_with("OK\n") {
        bail!("Command rejected or outcome unknown")
    }
    if matches!(r.op, 6 | 7) {
        let command = preview_command(&std::env::current_exe()?, &dir, r, &response)?;
        println!("execute_manually={command}");
        println!("No change made. Review scope/IP/TTL; run separately as the same service UID. TTL starts at apply. No automatic refresh or retry.");
    }
    Ok(())
}
pub async fn run() -> Result<()> {
    match Cli::parse().command {
        Command::InspectRuntime { control_dir } => {
            print!("{}", super::sockets::inspect(&control_dir)?);
            Ok(())
        }
        Command::Serve {
            systemd_notify,
            iface,
            xdp_mode,
            ebpf_object,
            control_dir,
            destination,
            port,
            protected_ip,
        } => {
            serve(
                iface,
                ebpf_object,
                control_dir,
                destination,
                port,
                protected_ip,
                systemd_notify,
                xdp_mode,
            )
            .await
        }
        Command::Status { control_dir, ip } => {
            client(
                control_dir,
                Request {
                    op: 0,
                    instance: [0; 16],
                    expected: 0,
                    ip: ip.map(u32::from).unwrap_or(0),
                    ttl: 0,
                },
            )
            .await
        }
        Command::List { control_dir } => {
            client(
                control_dir,
                Request {
                    op: 5,
                    instance: [0; 16],
                    expected: 0,
                    ip: 0,
                    ttl: 0,
                },
            )
            .await
        }
        Command::PreviewBan {
            control_dir,
            ip,
            ttl_secs,
        } => {
            client(
                control_dir,
                Request {
                    op: 6,
                    instance: [0; 16],
                    expected: 0,
                    ip: u32::from(ip),
                    ttl: ttl_secs,
                },
            )
            .await
        }
        Command::PreviewUnban { control_dir, ip } => {
            client(
                control_dir,
                Request {
                    op: 7,
                    instance: [0; 16],
                    expected: 0,
                    ip: u32::from(ip),
                    ttl: 0,
                },
            )
            .await
        }
        Command::Ban {
            control_dir,
            ip,
            ttl_secs,
            instance,
            expected_revision,
        } => {
            client(
                control_dir,
                Request {
                    op: 1,
                    instance: decode_instance(&instance)?,
                    expected: expected_revision,
                    ip: u32::from(ip),
                    ttl: ttl_secs,
                },
            )
            .await
        }
        Command::Unban {
            control_dir,
            ip,
            instance,
            expected_revision,
        } => {
            client(
                control_dir,
                Request {
                    op: 2,
                    instance: decode_instance(&instance)?,
                    expected: expected_revision,
                    ip: u32::from(ip),
                    ttl: 0,
                },
            )
            .await
        }
        Command::Bypass {
            control_dir,
            mode,
            instance,
            expected_revision,
        } => {
            client(
                control_dir,
                Request {
                    op: if mode == "on" { 3 } else { 4 },
                    instance: decode_instance(&instance)?,
                    expected: expected_revision,
                    ip: 0,
                    ttl: 0,
                },
            )
            .await
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xdp_startup_mode_is_fixed_explicit_and_strict() {
        let base = [
            "veil-local-ban",
            "serve",
            "--iface",
            "test0",
            "--ebpf-object",
            "unused.o",
            "--control-dir",
            "/unused",
            "--destination",
            "192.0.2.1",
            "--port",
            "443",
        ];
        let mode = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
            Command::Serve { xdp_mode, .. } => xdp_mode,
            _ => panic!("serve expected"),
        };
        assert_eq!(mode(&base), AttachMode::Default);
        let mut explicit = base.to_vec();
        explicit.extend(["--xdp-mode", "generic"]);
        assert_eq!(mode(&explicit), AttachMode::Generic);
        for invalid in ["auto", "native", "skb", "", "GENERIC"] {
            let mut args = base.to_vec();
            args.extend(["--xdp-mode", invalid]);
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
    struct MemoryBackend;
    impl Backend for MemoryBackend {
        fn insert(&self, _: u32, _: veil_common::PeerPolicy) -> Result<()> {
            Ok(())
        }
        fn remove(&self, _: u32) -> Result<()> {
            Ok(())
        }
        fn bypass(&self, _: bool) -> Result<()> {
            Ok(())
        }
        fn counters(&self) -> Result<(u64, u64)> {
            Ok((0, 0))
        }
    }
    fn memory_shared() -> Arc<Shared> {
        let backend: Arc<dyn Backend> = Arc::new(MemoryBackend);
        Arc::new(Shared {
            engine: Arc::new(Mutex::new(Engine::new(backend.clone(), &[]).unwrap())),
            mutations: Arc::new(Semaphore::new(1)),
            backend,
            instance: [1; 16],
            control: std::sync::Mutex::new(()),
            control_revision: AtomicU64::new(0),
            bypass: AtomicBool::new(false),
            destination: Ipv4Addr::new(192, 0, 2, 1),
            port: 443,
        })
    }
    fn read_only(op: u8, ip: u32, ttl: u32) -> Request {
        Request {
            op,
            instance: [0; 16],
            expected: 0,
            ip,
            ttl,
        }
    }
    async fn request(shared: Arc<Shared>, r: Request, emergency: bool) -> String {
        let (mut client, server) = UnixStream::pair().unwrap();
        client.write_all(&r.encode()).await.unwrap();
        client.shutdown().await.unwrap();
        let permit = Arc::new(Semaphore::new(8)).acquire_owned().await.unwrap();
        let handler = tokio::spawn(handle(server, shared, emergency, permit));
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        handler.await.unwrap();
        response
    }
    #[tokio::test]
    async fn list_transport_accepts_complete_snapshot_but_rejects_oversize_replies() {
        use std::os::unix::fs::DirBuilderExt;
        for (op, len, accepted) in [
            (5, 40_000, true),
            (5, LIST_RESPONSE_LIMIT + 1, false),
            (0, RESPONSE_LIMIT + 1, false),
        ] {
            let mut random = [0; 12];
            SystemRandom::new().fill(&mut random).unwrap();
            let dir = std::env::temp_dir().join(format!("vlb-list-{}", hex(&random)));
            std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
            let listener = UnixListener::bind(dir.join(NORMAL)).unwrap();
            let sender = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(read_request(&mut stream).await.unwrap().op, op);
                let _ = stream.write_all(&vec![b'x'; len]).await;
            });
            let result = exchange(&dir, read_only(op, 0, 0)).await;
            assert_eq!(result.is_ok(), accepted);
            if accepted {
                assert_eq!(result.unwrap().len(), len);
            }
            sender.await.unwrap();
            // Own test files are retained in the private temporary workspace.
        }
    }
    #[tokio::test]
    async fn list_is_complete_bounded_and_preserves_all_256_slots() {
        let shared = memory_shared();
        {
            let mut e = shared.engine.lock().await;
            for n in 0..CAPACITY {
                e.ban(
                    0xc6120001 + n as u32,
                    86_400,
                    n as u64,
                    STEP_NS * (n as u64 + 1),
                )
                .unwrap();
            }
        }
        let response = request(shared.clone(), read_only(5, 0, 0), false).await;
        assert!(response.starts_with("OK\n"));
        assert!(response.contains("entries=256\n"));
        assert!(response.contains("revision=256\n"));
        assert!(response.contains("destination=192.0.2.1\nprotocol=tcp\nport=443\n"));
        assert_eq!(
            response.lines().filter(|l| l.starts_with("entry=")).count(),
            CAPACITY
        );
        assert!(response.len() > RESPONSE_LIMIT && response.len() <= LIST_RESPONSE_LIMIT);
        let e = shared.engine.lock().await;
        assert_eq!((e.occupied(), e.revision()), (256, 256));
        assert_eq!(
            e.preview_ban(0xc6130001, 30, kernel::boot_now().unwrap()),
            Err("CAPACITY")
        );
    }
    #[tokio::test]
    async fn inspection_is_read_only_busy_is_not_an_empty_list_and_emergency_is_separate() {
        let shared = memory_shared();
        let ip = 0xc6120001;
        for r in [
            read_only(5, 0, 0),
            read_only(6, ip, 30),
            read_only(7, ip, 0),
        ] {
            let response = request(shared.clone(), r, false).await;
            assert!(response.starts_with("OK\n"), "{response}");
            assert_eq!(
                request(shared.clone(), r, true).await,
                err("WRONG_ENDPOINT")
            );
        }
        assert_eq!(
            request(shared.clone(), read_only(6, ip, 0), false).await,
            err("INVALID_TTL")
        );
        assert_eq!(
            request(shared.clone(), read_only(7, 0, 0), false).await,
            err("PROTECTED_OR_INVALID_IP")
        );
        let e = shared.engine.lock().await;
        assert_eq!((e.occupied(), e.revision(), e.event_count()), (0, 0, 0));
        assert_eq!(
            request(shared.clone(), read_only(5, 0, 0), false).await,
            err("BUSY")
        );
        assert_eq!(
            request(shared.clone(), read_only(6, ip, 30), false).await,
            err("BUSY")
        );
    }
    #[tokio::test]
    async fn preview_frozen_revision_and_old_instance_are_rejected_without_refresh() {
        let shared = memory_shared();
        let ip = 0xc6120001;
        let preview = request(shared.clone(), read_only(6, ip, 30), false).await;
        let command = preview_command(
            Path::new("/opt/veil-local-ban"),
            Path::new("/run/veil"),
            read_only(6, ip, 30),
            &preview,
        )
        .unwrap();
        assert!(command.contains("'--expected-revision' '0'"));
        shared
            .engine
            .lock()
            .await
            .ban(ip + 1, 30, 0, STEP_NS)
            .unwrap();
        let stale = Request {
            op: 1,
            instance: shared.instance,
            expected: 0,
            ip,
            ttl: 30,
        };
        assert!(request(shared.clone(), stale, false)
            .await
            .contains("code=REVISION_CONFLICT\n"));
        assert_eq!(shared.engine.lock().await.deadline(ip), None);
        let mut restarted = memory_shared();
        Arc::get_mut(&mut restarted).unwrap().instance = [2; 16];
        assert_eq!(
            request(restarted.clone(), stale, false).await,
            err("INSTANCE_CONFLICT")
        );
        let listing = request(restarted, read_only(5, 0, 0), false).await;
        assert!(listing.contains("entries=0\n") && listing.contains("revision=0\n"));
        assert!(command.contains(&format!("'{}'", hex(&shared.instance))));
    }
    #[tokio::test]
    async fn preview_apply_is_explicit_and_replay_cannot_restore_a_released_ban() {
        let shared = memory_shared();
        let ip = 0xc6120001;
        assert!(request(shared.clone(), read_only(6, ip, 30), false)
            .await
            .contains("preview=true\n"));
        assert_eq!(shared.engine.lock().await.occupied(), 0);
        let ban = Request {
            op: 1,
            instance: shared.instance,
            expected: 0,
            ip,
            ttl: 30,
        };
        assert!(request(shared.clone(), ban, false)
            .await
            .contains("result=APPLIED\n"));
        let before = shared.engine.lock().await.entries();
        let preview = request(shared.clone(), read_only(7, ip, 0), false).await;
        assert!(preview.contains("operation=unban\n") && preview.contains("revision=1\n"));
        assert_eq!(before, shared.engine.lock().await.entries());
        tokio::time::sleep(Duration::from_millis(210)).await;
        let unban = Request {
            op: 2,
            instance: shared.instance,
            expected: 1,
            ip,
            ttl: 0,
        };
        assert!(request(shared.clone(), unban, false)
            .await
            .contains("result=APPLIED\n"));
        assert!(request(shared.clone(), ban, false)
            .await
            .contains("code=REVISION_CONFLICT\n"));
        assert_eq!(shared.engine.lock().await.occupied(), 0);
    }
    #[test]
    fn preview_commands_quote_paths_and_reject_ambiguous_or_changed_responses() {
        let shared = memory_shared();
        let r = read_only(6, 0xc6120001, 30);
        let response = shared.inspect_policy(r);
        let command = preview_command(
            Path::new("/opt/veil local'ban"),
            Path::new("/run/a'$(printf injected)"),
            r,
            &response,
        )
        .unwrap();
        assert!(command.starts_with("'/opt/veil local'\"'\"'ban' 'ban'"));
        assert!(command.contains("'/run/a'\"'\"'$(printf injected)'"));
        let roundtrip = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("set -- {command}; printf '%s\\0' \"$@\""))
            .output()
            .unwrap();
        assert!(roundtrip.status.success());
        let args: Vec<_> = roundtrip
            .stdout
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| std::str::from_utf8(b).unwrap())
            .collect();
        assert_eq!(args[0], "/opt/veil local'ban");
        assert_eq!(args[3], "/run/a'$(printf injected)");
        assert!(matches!(
            Cli::try_parse_from(args).unwrap().command,
            Command::Ban {
                expected_revision: 0,
                ttl_secs: 30,
                ..
            }
        ));
        assert!(preview_command(
            Path::new("/veil"),
            Path::new("/run/a\ncommand"),
            r,
            &response
        )
        .is_err());
        for bad in [
            response.replace("ttl_secs=30", "ttl_secs=31"),
            response.replace("ip=198.18.0.1", "ip=198.18.0.2"),
            response.replace("revision=0", "revision=x"),
            response.replace("operation=ban", "operation=unban"),
            response.clone() + "revision=0\n",
            err("INVALID_FRAME"),
        ] {
            assert!(preview_command(Path::new("/veil"), Path::new("/run/veil"), r, &bad).is_err());
        }
        let unban = read_only(7, r.ip, 0);
        let command = preview_command(
            Path::new("/veil"),
            Path::new("/run/veil"),
            unban,
            &shared.inspect_policy(unban),
        )
        .unwrap();
        assert!(command.contains("'unban'"));
        assert!(!command.contains("--ttl-secs"));
    }
    #[test]
    fn inspection_frames_are_strict_and_cli_requires_explicit_mutation_tokens() {
        for r in [
            read_only(5, 0, 0),
            read_only(6, 0xc6120001, 30),
            read_only(7, 0xc6120001, 0),
        ] {
            assert!(Request::parse(&r.encode()).is_ok());
            for bad in [
                Request {
                    instance: [1; 16],
                    ..r
                },
                Request { expected: 1, ..r },
            ] {
                assert!(Request::parse(&bad.encode()).is_err());
            }
        }
        assert!(Request::parse(&read_only(5, 1, 0).encode()).is_err());
        assert!(Request::parse(&read_only(7, 1, 1).encode()).is_err());
        assert!(
            Cli::try_parse_from(["veil-local-ban", "list", "--control-dir", "/run/veil"]).is_ok()
        );
        assert!(Cli::try_parse_from([
            "veil-local-ban",
            "preview-ban",
            "--control-dir",
            "/run/veil",
            "--ip",
            "198.18.0.1",
            "--ttl-secs",
            "30"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "veil-local-ban",
            "ban",
            "--control-dir",
            "/run/veil",
            "--ip",
            "198.18.0.1",
            "--ttl-secs",
            "30"
        ])
        .is_err());
    }
    async fn submit_mutation(shared: Arc<Shared>) -> (UnixStream, tokio::task::JoinHandle<()>) {
        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .write_all(
                &Request {
                    op: 1,
                    instance: shared.instance,
                    expected: 0,
                    ip: u32::from(Ipv4Addr::new(198, 18, 0, 1)),
                    ttl: 30,
                }
                .encode(),
            )
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let permit = Arc::new(Semaphore::new(8)).acquire_owned().await.unwrap();
        (client, tokio::spawn(handle(server, shared, false, permit)))
    }
    #[tokio::test]
    async fn one_mutation_waits_for_sweep_and_competitor_is_rejected() {
        let shared = memory_shared();
        let sweep_guard = shared.engine.clone().lock_owned().await;
        let (mut first, task) = submit_mutation(shared.clone()).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while shared.mutations.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let (mut second, other) = submit_mutation(shared.clone()).await;
        let mut response = String::new();
        second.read_to_string(&mut response).await.unwrap();
        other.await.unwrap();
        assert_eq!(response, err("BUSY"));
        assert_eq!(sweep_guard.occupied(), 0);
        drop(sweep_guard);
        response.clear();
        first.read_to_string(&mut response).await.unwrap();
        task.await.unwrap();
        assert!(response.contains("result=APPLIED"));
        assert_eq!(shared.engine.lock().await.occupied(), 1);
        assert_eq!(shared.mutations.available_permits(), 1);
    }
    #[tokio::test]
    async fn timed_out_mutation_keeps_single_admission_until_worker_finishes() {
        let shared = memory_shared();
        let sweep_guard = shared.engine.clone().lock_owned().await;
        let (mut first, task) = submit_mutation(shared.clone()).await;
        let mut response = String::new();
        first.read_to_string(&mut response).await.unwrap();
        task.await.unwrap();
        assert_eq!(response, err("OUTCOME_UNKNOWN_CHECK_STATUS"));
        assert_eq!(shared.mutations.available_permits(), 0);
        let (mut second, other) = submit_mutation(shared.clone()).await;
        response.clear();
        second.read_to_string(&mut response).await.unwrap();
        other.await.unwrap();
        assert_eq!(response, err("BUSY"));
        drop(sweep_guard);
        tokio::time::timeout(Duration::from_secs(1), async {
            while shared.mutations.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(shared.engine.lock().await.occupied(), 1);
    }
    #[test]
    fn queued_sweep_does_not_reserve_the_policy_lock() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let shared = memory_shared();
            let (started, started_rx) = tokio::sync::oneshot::channel();
            let (release, release_rx) = std::sync::mpsc::channel();
            let occupied = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            });
            started_rx.await.unwrap();
            let (stop, rx) = tokio::sync::watch::channel(false);
            let sweep = tokio::spawn(sweep_loop(shared.clone(), rx));
            // Let the interval's first timer tick be driven while the only
            // blocking worker is occupied. Merely yielding need not drive it.
            tokio::time::sleep(Duration::from_millis(10)).await;
            let available = shared.engine.try_lock().is_ok();
            // Release and join before asserting, even for the broken behavior.
            stop.send(true).unwrap();
            release.send(()).unwrap();
            occupied.await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), sweep)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(available, "queued sweep reserved the policy lock");
        });
    }
    #[test]
    fn fixed_frame_and_instance_validation() {
        let r = Request {
            op: 1,
            instance: [42; 16],
            expected: 17,
            ip: 0x0a000001,
            ttl: 4,
        };
        let bytes = r.encode();
        let decoded = Request::parse(&bytes).unwrap();
        assert_eq!(decoded.expected, 17);
        assert_eq!(decoded.instance, [42; 16]);
        assert!(Request::parse(&bytes[..39]).is_err());
        let mut extra = bytes.to_vec();
        extra.push(0);
        assert!(Request::parse(&extra).is_err());
        let mut bad = bytes;
        bad[7] = 1;
        assert!(Request::parse(&bad).is_err());
        bad = bytes;
        bad[4] = 9;
        assert!(Request::parse(&bad).is_err());
        assert_eq!(decode_instance(&hex(&[42; 16])).unwrap(), [42; 16]);
        assert!(decode_instance("z").is_err());
        assert!(decode_instance(&"é".repeat(16)).is_err());
        bad = bytes;
        bad[4] = 0;
        assert!(Request::parse(&bad).is_err());
        bad = bytes;
        bad[4] = 3;
        assert!(Request::parse(&bad).is_err());
    }
    #[tokio::test]
    async fn framing_requires_eof_and_rejects_trailing_bytes() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let bytes = Request {
            op: 0,
            instance: [0; 16],
            expected: 0,
            ip: 0,
            ttl: 0,
        }
        .encode();
        client.write_all(&bytes).await.unwrap();
        client.write_all(&[0]).await.unwrap();
        client.shutdown().await.unwrap();
        assert!(read_request(&mut server).await.is_err());
    }

    #[tokio::test]
    #[ignore = "explicit subprocess fixture for isolated SIGKILL acceptance"]
    async fn initialization_crash_fixture() {
        assert_eq!(
            std::env::var("VEIL_LOCAL_BAN_INIT_FIXTURE").as_deref(),
            Ok("1")
        );
        let dir = std::env::var_os("VEIL_LOCAL_BAN_INIT_DIR").expect("fixture directory");
        let object = std::env::var("VEIL_LOCAL_BAN_INIT_OBJECT").expect("fixture object");
        serve(
            "veth-rogue".into(),
            object,
            dir.into(),
            Ipv4Addr::new(10, 99, 0, 66),
            18443,
            vec![],
            false,
            AttachMode::Default,
        )
        .await
        .expect("fixture startup");
        panic!("fixture must be stopped at a checkpoint and killed by its parent");
    }
}
