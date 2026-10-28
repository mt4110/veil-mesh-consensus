// ==============================================================================
// veil-node: Clean Architecture Composition Root
// - domain:   Enterprise Domain Entities & Guardrail Security Policies
// - usecase:  ConsensusState Engine & Output Port Traits (`EbpfBridge`)
// - adapter:  Ring Ed25519 CSPRNG Identity (`0600` KeyFile) & Wire Protocol
// - infra:    Quinn mTLS 1.3 Transport, Aya Linux Kernel eBPF, Ratatui TUI Arena
// ==============================================================================

pub mod adapter;
pub mod consensus;
pub mod domain;
pub mod ebpf_bridge;
pub mod infra;
pub mod tui;
pub mod usecase;

#[cfg(test)]
mod proof_tests;

#[cfg(all(test, target_os = "linux"))]
mod ttl_linux_tests;

use adapter::NodeCryptoIdentity;
use anyhow::Result;
use clap::Parser;
use infra::{handle_authenticated_connection, node_tls::NodeTls};
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use tracing::{info, warn};
use usecase::ConsensusState;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "veil-node",
    group(clap::ArgGroup::new("operation").args(["init_approver", "approve_ban_ip", "approve_unban_ip", "submit_approval", "init_key", "init_state", "print_tls_name", "check_tls", "bypass_local", "ban_ip", "request_ban_ip", "status_local", "tui", "tui_demo"]).multiple(false)),
    about = "veil-mesh-consensus QUIC & eBPF node daemon (Clean Architecture)"
)]
struct Args {
    /// Node identifier (e.g. `node-alpha`)
    #[arg(long, default_value = "node-alpha")]
    node_id: String,

    /// Local QUIC UDP bind address
    #[arg(long, default_value = "0.0.0.0:5000")]
    listen: SocketAddr,

    /// Complete voter list, including self: --member ID@IP:PORT=HEX_PUBLIC_KEY (repeat).
    #[arg(long)]
    member: Vec<domain::Member>,

    /// Additional protected exact IPv4 addresses, identical on every member.
    #[arg(long)]
    protected_ip: Vec<Ipv4Addr>,

    /// Independent off-node operator public key, identical on all validators.
    #[arg(long)]
    approval_public_key: Option<String>,
    /// Repeat exact service destination/protocol/port, e.g. 192.0.2.10/tcp/443.
    #[arg(long)]
    scope: Vec<domain::ServiceScope>,
    /// Create a new off-node operator key exclusively; never starts a daemon.
    #[arg(long, requires = "approval_key_file")]
    init_approver: bool,
    /// Operator key, used only by offline initialization/signing operations.
    #[arg(long)]
    approval_key_file: Option<PathBuf>,
    /// Produce an approval for a BAN; requires the complete public configuration.
    #[arg(long, group="ban_submission", requires_all=["ttl_secs","approval_key_file","approval_output","expected_policy_revision"], conflicts_with="approve_unban_ip")]
    approve_ban_ip: Option<Ipv4Addr>,
    /// Produce an individual unban approval; no BAN expiry is renewed.
    #[arg(long, requires_all=["approval_key_file","approval_output","expected_policy_revision"])]
    approve_unban_ip: Option<Ipv4Addr>,
    #[arg(long)]
    expected_policy_revision: Option<u64>,
    #[arg(long)]
    approval_output: Option<PathBuf>,
    /// Submit an operator-signed operation through any member's local socket.
    #[arg(long, requires = "control_socket")]
    submit_approval: Option<PathBuf>,

    /// Explicitly provision a signing key and print its public key; never starts a daemon.
    #[arg(long, requires = "key_file", conflicts_with_all = ["tui", "tui_demo", "bypass_local"])]
    init_key: bool,

    /// Create the initial WAL once, using permission issued by --init-key; never starts a daemon.
    #[arg(long, requires_all = ["key_file", "wal_file"], conflicts_with_all = ["init_key", "print_tls_name", "check_tls", "tui", "tui_demo", "bypass_local", "ban_ip", "request_ban_ip", "status_local", "ebpf_obj", "control_socket"])]
    init_state: bool,

    /// Unique cluster namespace for all consensus signatures (required for daemon mode).
    #[arg(long)]
    cluster_id: Option<String>,

    /// Static configuration generation; must increase when changing configuration.
    #[arg(long)]
    config_epoch: Option<u64>,

    /// Print the expected TLS SAN for this node and fixed configuration; no keys are read.
    #[arg(long, conflicts_with_all = ["init_key", "tui", "tui_demo", "bypass_local", "ban_ip", "request_ban_ip", "status_local"])]
    print_tls_name: bool,

    /// Validate a provisioned TLS bundle and print its public fingerprint; never starts a daemon.
    #[arg(long, requires = "tls_dir", conflicts_with_all = ["print_tls_name", "init_key", "tui", "tui_demo", "bypass_local", "ban_ip", "request_ban_ip", "status_local"])]
    check_tls: bool,

    /// Read-only 0700 node TLS bundle; contains no CA private key (required for daemon).
    #[arg(long)]
    tls_dir: Option<PathBuf>,

    /// Legacy state directory for signing key/WAL defaults; never a TLS credential source
    #[arg(long)]
    cluster_dir: Option<PathBuf>,

    /// Existing Ed25519 key file for daemon mode; --init-key explicitly provisions it
    #[arg(long)]
    key_file: Option<PathBuf>,

    /// Path to persistent Crash-Recovery WAL file (`0600`)
    #[arg(long)]
    wal_file: Option<PathBuf>,

    /// Local hard WAL limit, including the envelope; reaching it stops voting.
    #[arg(long, default_value_t = domain::DEFAULT_MAX_WAL_BYTES, value_parser = clap::value_parser!(u64).range(65536..=268435456))]
    max_wal_bytes: u64,

    /// Maximum kernel-reported clock error allowed for new TTL leases (milliseconds).
    #[arg(long, default_value_t = 1_000)]
    max_clock_error_ms: u64,

    /// Target network interface for XDP attachment (default: eth1)
    #[arg(long, default_value = "eth1")]
    iface: String,

    /// Path to compiled eBPF object file (optional on non-Linux / dev mode)
    #[arg(long)]
    ebpf_obj: Option<String>,

    /// Send a local XDP bypass request to a running daemon (requires --control-socket).
    #[arg(long, value_parser = ["on", "off"], requires = "control_socket")]
    bypass_local: Option<String>,

    /// Submit a finite exact-IPv4 BAN to the local leader (receipt means accepted, not committed).
    #[arg(long, requires_all = ["control_socket", "ttl_secs"], conflicts_with_all = ["bypass_local", "status_local", "tui", "tui_demo", "init_key"])]
    #[arg(group = "ban_submission")]
    ban_ip: Option<Ipv4Addr>,

    /// Durably witness a finite BAN on any member; allows quorum progress timeout and handoff.
    #[arg(long, group = "ban_submission", requires_all = ["control_socket", "ttl_secs"], conflicts_with_all = ["ban_ip", "bypass_local", "status_local", "tui", "tui_demo", "init_key"])]
    request_ban_ip: Option<Ipv4Addr>,

    /// BAN lifetime in seconds, including consensus/network delay (1..=86400).
    #[arg(long, requires = "ban_submission", value_parser = clap::value_parser!(u32).range(1..=86_400))]
    ttl_secs: Option<u32>,

    /// Print local consensus/apply status; never treats acceptance as commit.
    #[arg(long, requires = "control_socket", conflicts_with_all = ["bypass_local", "tui", "tui_demo", "init_key"])]
    status_local: bool,

    /// Inspect a receipt's log index and compare its returned hash with the receipt.
    #[arg(long, requires = "status_local", value_parser = clap::value_parser!(u64).range(1..))]
    entry_index: Option<u64>,

    /// Inspect a witnessed request receipt (queued/logged/committed/applied/expired).
    #[arg(long, requires = "status_local", conflicts_with_all = ["entry_index", "status_ip"])]
    request_id: Option<String>,

    /// Include the local policy mirror verdict for this exact IPv4 address.
    #[arg(long, requires = "status_local")]
    status_ip: Option<Ipv4Addr>,

    /// Owner-only Unix control socket. Daemon defaults to a fresh private directory.
    #[arg(long)]
    control_socket: Option<PathBuf>,

    /// Launch interactive TUI Arena
    #[arg(long)]
    tui: bool,

    /// Run automated TUI demo scenario and exit
    #[arg(long)]
    tui_demo: bool,
}

fn daemon_configuration(args: &Args) -> Result<domain::FixedConfiguration> {
    let config = domain::FixedConfiguration::new(
        args.cluster_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("--cluster-id is required for daemon mode"))?,
        args.config_epoch
            .ok_or_else(|| anyhow::anyhow!("--config-epoch is required for daemon mode"))?,
        args.member.clone(),
        args.protected_ip.clone(),
        args.max_clock_error_ms,
    )
    .map_err(anyhow::Error::msg)?;
    let config = config
        .with_approval(domain::ApprovalPolicy {
            public_key: domain::parse_public_key(args.approval_public_key.as_deref().ok_or_else(
                || anyhow::anyhow!("--approval-public-key is required for daemon configuration"),
            )?)
            .map_err(anyhow::Error::msg)?,
            scopes: args.scope.clone(),
        })
        .map_err(anyhow::Error::msg)?;
    let local = config
        .members()
        .iter()
        .find(|m| m.node_id == args.node_id)
        .ok_or_else(|| anyhow::anyhow!("Local node must appear exactly once in --member"))?;
    anyhow::ensure!(
        args.listen.port() == local.address.port()
            && args.listen.is_ipv4() == local.address.is_ipv4()
            && (args.listen.ip().is_unspecified() || args.listen.ip() == local.address.ip()),
        "--listen must match the local member endpoint (a wildcard bind IP is allowed)"
    );
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "veil_node=info".into()),
        )
        .init();

    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = Args::parse();

    if args.init_approver {
        let path = args.approval_key_file.as_deref().expect("clap key");
        let key = NodeCryptoIdentity::create_new_file("operator".into(), path)?;
        println!(
            "approver {}",
            key.public_key_bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        return Ok(());
    }
    if let Some(ip) = args.approve_ban_ip.or(args.approve_unban_ip) {
        let config = daemon_configuration(&args)?;
        let a = infra::approval_cli::sign(
            &config,
            args.approval_key_file.as_deref().expect("key"),
            ip,
            args.approve_unban_ip.is_some(),
            args.ttl_secs.unwrap_or(0),
            args.expected_policy_revision.expect("revision"),
        )?;
        infra::approval_cli::write(args.approval_output.as_deref().expect("output"), &a)?;
        println!(
            "APPROVED revision={} ip={} action={} not_after_unix_ns={} valid_until_unix_ns={}",
            a.revision,
            ip,
            if a.rule.rule.trusted { "UNBAN" } else { "BAN" },
            a.rule.not_after_unix_ns,
            a.valid_until_unix_ns
        );
        return Ok(());
    }
    if let Some(path) = &args.submit_approval {
        let a = infra::approval_cli::read(path)?;
        print!(
            "{}",
            infra::local_control::submit_approval(
                args.control_socket.as_deref().expect("socket"),
                &a
            )
            .await?
        );
        return Ok(());
    }
    anyhow::ensure!(
        args.approval_key_file.is_none(),
        "Operator private key is forbidden in daemon/client mode"
    );
    if args.init_key {
        anyhow::ensure!(domain::valid_node_id(&args.node_id), "Invalid node ID");
        let identity = infra::bootstrap::provision_key(
            &args.node_id,
            args.key_file.as_deref().expect("clap requires key file"),
        )?;
        let public_key: String = identity
            .public_key_bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        println!("{} {}", args.node_id, public_key);
        return Ok(());
    }

    if let Some(mode) = &args.bypass_local {
        infra::local_control::request(
            args.control_socket
                .as_deref()
                .expect("clap requires socket"),
            mode == "on",
        )
        .await?;
        info!(mode, "Local XDP bypass durably acknowledged");
        return Ok(());
    }

    if let Some(ip) = args.request_ban_ip {
        let reply = infra::local_control::submit_ban_request(
            args.control_socket
                .as_deref()
                .expect("clap requires socket"),
            ip,
            args.ttl_secs.expect("clap requires TTL"),
        )
        .await?;
        print!("{reply}");
        return Ok(());
    }
    if let Some(id) = &args.request_id {
        let reply = infra::local_control::request_status(
            args.control_socket
                .as_deref()
                .expect("clap requires socket"),
            id,
        )
        .await?;
        print!("{reply}");
        return Ok(());
    }
    if let Some(ip) = args.ban_ip {
        let reply = infra::local_control::submit_ban(
            args.control_socket
                .as_deref()
                .expect("clap requires socket"),
            ip,
            args.ttl_secs.expect("clap requires TTL"),
        )
        .await?;
        print!("{reply}");
        return Ok(());
    }
    if args.status_local {
        let reply = infra::local_control::status(
            args.control_socket
                .as_deref()
                .expect("clap requires socket"),
            args.entry_index,
            args.status_ip,
        )
        .await?;
        print!("{reply}");
        return Ok(());
    }

    if args.tui || args.tui_demo {
        #[allow(unused_mut)]
        let mut real_bridge: Option<Arc<dyn usecase::EbpfBridge>> = None;
        #[cfg(target_os = "linux")]
        if let Some(ref obj_path) = args.ebpf_obj {
            real_bridge = Some(Arc::new(ebpf_bridge::AyaKernelBridge::load_and_attach(
                obj_path,
                &args.iface,
            )?));
        }
        return tui::run_tui(args.tui_demo, real_bridge, args.iface);
    }

    let configuration = daemon_configuration(&args)?;
    if args.print_tls_name {
        println!(
            "{}",
            infra::node_tls::identity_name(configuration.context(), &args.node_id)
        );
        return Ok(());
    }
    if args.init_state {
        let key = args.key_file.as_deref().expect("clap requires key file");
        let wal = args.wal_file.as_deref().expect("clap requires WAL file");
        let _identity_lock = infra::bootstrap::lock_private_file(key)?;
        let identity = NodeCryptoIdentity::load_existing_file(&args.node_id, key)?;
        let mut state = ConsensusState::new(&args.node_id, vec![]);
        state
            .configure_fixed_membership(configuration, identity)
            .map_err(anyhow::Error::msg)?;
        state.enable_two_phase().map_err(anyhow::Error::msg)?;
        state.max_wal_bytes = args.max_wal_bytes;
        match std::fs::symlink_metadata(wal) {
            Ok(_) => anyhow::bail!("Initial WAL must not exist; refusing replacement"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        infra::bootstrap::consume_permission(key, state.identity())?;
        let store = Arc::new(infra::wal::FileWal::create_new(wal)?.with_limit(args.max_wal_bytes));
        state
            .enable_persistence(store)
            .map_err(anyhow::Error::msg)?;
        println!("OK initial WAL created; preserve it with this signing identity");
        return Ok(());
    }
    let tls = Arc::new(NodeTls::load(
        args.tls_dir.as_deref().ok_or_else(|| {
            anyhow::anyhow!("--tls-dir is required; provision a node bundle offline")
        })?,
        &configuration,
        &args.node_id,
    )?);
    if args.check_tls {
        println!(
            "OK node={} tls_name={} certificate_sha256={}",
            args.node_id,
            infra::node_tls::identity_name(configuration.context(), &args.node_id),
            tls.certificate_sha256
        );
        return Ok(());
    }
    info!(certificate_sha256 = %tls.certificate_sha256, "Validated node TLS certificate");
    let peer_addrs = configuration.peer_addrs(&args.node_id);
    let cluster_dir = args
        .cluster_dir
        .clone()
        .or_else(|| std::env::var_os("VEIL_CLUSTER_PKI_DIR").map(PathBuf::from))
        .unwrap_or_else(|| std::env::temp_dir().join("veil-mesh-cluster-pki-v2"));

    info!(
        node_id = %args.node_id,
        listen = %args.listen,
        peers = ?peer_addrs,
        context = ?configuration.context(),
        cluster_dir = %cluster_dir.display(),
        iface = %args.iface,
        "Starting veil-node daemon"
    );

    let key_path = args.key_file.clone().unwrap_or_else(|| {
        cluster_dir
            .join("keys")
            .join(format!("{}.pk8", args.node_id))
    });
    let _identity_lock = infra::bootstrap::lock_private_file(&key_path)?;
    let identity = NodeCryptoIdentity::load_existing_file(&args.node_id, &key_path)?;
    let mut consensus_state = ConsensusState::new(args.node_id.clone(), vec![]);
    consensus_state
        .configure_fixed_membership(configuration, identity)
        .map_err(anyhow::Error::msg)?;
    consensus_state
        .enable_two_phase()
        .map_err(anyhow::Error::msg)?;
    consensus_state.max_wal_bytes = args.max_wal_bytes;
    #[cfg(target_os = "linux")]
    consensus_state.configure_policy_clock(Arc::new(
        infra::policy_clock::LinuxBootClock::with_max_error_ms(args.max_clock_error_ms)?,
    ))?;
    #[cfg(not(target_os = "linux"))]
    consensus_state.configure_policy_clock(Arc::new(
        infra::policy_clock::UnavailableLeaseClock(std::time::Instant::now()),
    ))?;
    info!(path = %key_path.display(), "Loaded fixed membership signing identity");

    let wal_path = args.wal_file.clone().unwrap_or_else(|| {
        cluster_dir
            .join("wal")
            .join(format!("{}.wal", args.node_id))
    });
    let wal_store =
        Arc::new(infra::wal::FileWal::open_existing(&wal_path)?.with_limit(args.max_wal_bytes));

    anyhow::ensure!(
        std::fs::metadata(&wal_path)?.len() <= args.max_wal_bytes,
        "Existing WAL exceeds configured byte limit; preserve history"
    );
    #[cfg(target_os = "linux")]
    if let Some(ref obj_path) = args.ebpf_obj {
        match ebpf_bridge::AyaKernelBridge::load_and_attach_with_scopes(
            obj_path,
            &args.iface,
            &args.scope,
        ) {
            Ok(bridge) => {
                consensus_state = consensus_state.with_ebpf_bridge(Arc::new(bridge));
                info!("Connected ConsensusState to Linux kernel Aya eBPF maps (`PEER_POLICY_MAP` & `LSM_POLICY_MAP`)");
            }
            Err(err) => {
                return Err(err.context("Requested XDP attachment failed"));
            }
        }
    }

    consensus_state.load_wal(&wal_path)?;
    info!(path = %wal_path.display(), term = consensus_state.current_term, commit_index = consensus_state.commit_index, "Restored ConsensusState from Crash-Recovery WAL");
    consensus_state
        .enable_persistence(wal_store)
        .map_err(anyhow::Error::msg)?;

    let endpoint = tls.make_endpoint(args.listen)?;
    let state = Arc::new(Mutex::new(consensus_state));
    let control_path = if let Some(path) = args.control_socket {
        path
    } else {
        use std::os::unix::fs::DirBuilderExt;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("veil-control-{}-{stamp}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        dir.join("node.sock")
    };
    let control_listener = infra::local_control::bind(&control_path)?;
    info!(path = %control_path.display(), "Owner-only local control socket");
    let control_state = state.clone();
    let mut control_task =
        tokio::spawn(infra::local_control::serve(control_listener, control_state));
    let mut driver_task = tokio::spawn(infra::daemon::run(
        endpoint.clone(),
        state.clone(),
        peer_addrs,
        tls.clone(),
    ));

    let apply_state = state.clone();
    let apply_task = tokio::spawn(async move {
        let mut retry = tokio::time::interval(Duration::from_millis(200));
        loop {
            retry.tick().await;
            if let Err(error) =
                infra::state_worker::run(apply_state.clone(), |st| st.retry_pending_apply()).await
            {
                tracing::error!(%error, "Apply worker stopped");
                break;
            }
        }
    });

    let connections = Arc::new(tokio::sync::Semaphore::new(128));
    loop {
        let incoming = tokio::select! {
            result = &mut driver_task => {
                anyhow::bail!("Consensus driver stopped: {result:?}");
            }
            result = &mut control_task => {
                anyhow::bail!("Local control server stopped: {result:?}");
            }
            result = tokio::signal::ctrl_c() => {
                result?;
                break;
            }
            incoming = endpoint.accept() => match incoming {
                Some(incoming) => incoming,
                None => break,
            }
        };
        let Ok(slot) = connections.clone().try_acquire_owned() else {
            incoming.refuse();
            continue;
        };
        let node_id = args.node_id.clone();
        let state = state.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            let _slot = slot;
            match tokio::time::timeout(Duration::from_secs(2), incoming).await {
                Ok(Ok(conn)) => {
                    if let Err(err) =
                        handle_authenticated_connection(conn, &node_id, state, tls).await
                    {
                        warn!(error = %err, "Connection handler error");
                    }
                }
                result => {
                    warn!(error = ?result, "Incoming QUIC mTLS handshake failed or timed out")
                }
            }
        });
    }
    endpoint.close(0u32.into(), b"daemon shutdown");
    driver_task.abort();
    control_task.abort();
    apply_task.abort();

    Ok(())
}

#[cfg(test)]
mod startup_tests;
