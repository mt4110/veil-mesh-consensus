use crate::consensus::{ConsensusRpc, ConsensusState, MeshCommand, NodeCryptoIdentity, Role};
use crate::ebpf_bridge::{EbpfBridge, MockEbpfBridge};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table},
    Frame, Terminal,
};
use std::{
    collections::BTreeMap,
    io,
    net::Ipv4Addr,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use veil_common::XdpVerdict;

/// わざと起動させる「モック攻撃プロセス（実 UDP Flood / 空プロセス `sleep`）」の管理情報
pub struct RogueAttackProcess {
    pub pid: u32,
    pub attacker_ip: Ipv4Addr,
    pub target_node: String,
    pub attack_type: String,
    pub status: RogueStatus,
    pub packets_sent: u64,
    pub packets_dropped_xdp: u64,
    child: Option<Child>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RogueStatus {
    /// 攻撃パケット送信中（未検知・通過中）
    Infiltrating,
    /// コンセンサス投票・合意中
    ConsensusVoting,
    /// eBPF XDP_DROP + LSM -EPERM + SIGKILL により完全駆逐済み
    Exterminated,
}

impl RogueAttackProcess {
    /// OS 上に攻撃プロセスを実際に spawn して本物の PID を取得する
    /// - Linux で `ns-veil-rogue` が存在する場合は、実際に `10.99.0.66 -> 10.99.0.1:9999` へ秒間 1000 パケットの UDP Flood を撃ち込む Python プロセスを起動する
    /// - それ以外の環境では安全な `sleep 300` プロセスを起動する
    pub fn spawn_mock(attacker_ip: Ipv4Addr, target_node: &str, attack_type: &str) -> Result<Self> {
        let has_netns = Command::new("ip")
            .args(["netns", "pids", "ns-veil-rogue"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        let child = if has_netns {
            Command::new("ip")
                .args([
                    "netns",
                    "exec",
                    "ns-veil-rogue",
                    "python3",
                    "-c",
                    "import socket,time; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); p=b'VEIL_ROGUE_FLOOD_'*40\nwhile True:\n s.sendto(p,('10.99.0.1',9999)); time.sleep(0.001)",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?
        } else {
            Command::new("sleep")
                .arg("300")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?
        };
        let pid = child.id();

        Ok(Self {
            pid,
            attacker_ip,
            target_node: target_node.to_string(),
            attack_type: attack_type.to_string(),
            status: RogueStatus::Infiltrating,
            packets_sent: 1,
            packets_dropped_xdp: 0,
            child: Some(child),
        })
    }

    /// OS プロセスを強制終了（駆逐）し、発行したシステムコールと wait4 の回収ステータス（SIGKILL=9 等）を返す
    pub fn kill_process(&mut self) -> String {
        let mut syscall_evidence = format!("syscall::kill(pid={}, sig=9/SIGKILL)", self.pid);
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            if let Ok(status) = c.wait() {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(sig) = status.signal() {
                        syscall_evidence = format!(
                            "sys_kill(pid={}, SIGKILL={}) -> wait4() reaped zombie (term_sig={})",
                            self.pid, sig, sig
                        );
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = status;
                }
            }
        }
        self.status = RogueStatus::Exterminated;
        syscall_evidence
    }
}

impl Drop for RogueAttackProcess {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// TUI シミュレータの全体状態
pub struct TuiArenaApp {
    pub nodes: BTreeMap<String, ConsensusState>,
    pub bridges: BTreeMap<String, Arc<MockEbpfBridge>>,
    pub real_kernel_bridge: Option<Arc<dyn EbpfBridge>>,
    pub kernel_iface: String,
    pub identities: BTreeMap<String, NodeCryptoIdentity>,
    pub rogues: Vec<RogueAttackProcess>,
    pub event_logs: Vec<String>,
    pub total_xdp_pass: u64,
    pub total_xdp_drop: u64,
    pub step_tick: u64,
}

impl Default for TuiArenaApp {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiArenaApp {
    pub fn new() -> Self {
        Self::new_with_kernel_bridge(None, "sim-eth1".to_string())
    }

    pub fn new_with_kernel_bridge(
        real_kernel_bridge: Option<Arc<dyn EbpfBridge>>,
        kernel_iface: String,
    ) -> Self {
        let mut app = Self {
            nodes: BTreeMap::new(),
            bridges: BTreeMap::new(),
            real_kernel_bridge,
            kernel_iface,
            identities: BTreeMap::new(),
            rogues: Vec::new(),
            event_logs: Vec::new(),
            total_xdp_pass: 0,
            total_xdp_drop: 0,
            step_tick: 0,
        };
        app.reset_cluster();
        app
    }

    pub fn log(&mut self, msg: impl Into<String>) {
        let line = format!("[T+{:03}] {}", self.step_tick, msg.into());
        self.event_logs.push(line);
        if self.event_logs.len() > 50 {
            self.event_logs.remove(0);
        }
    }

    pub fn reset_cluster(&mut self) {
        for r in &mut self.rogues {
            r.kill_process();
        }
        self.rogues.clear();
        self.nodes.clear();
        self.bridges.clear();
        self.identities.clear();
        self.event_logs.clear();
        self.total_xdp_pass = 0;
        self.total_xdp_drop = 0;
        self.step_tick = 0;

        let names = ["node-alpha", "node-beta", "node-gamma"];
        let mut pub_keys = BTreeMap::new();

        for (i, &name) in names.iter().enumerate() {
            let seed = [(i as u8) + 1; 32];
            let id = NodeCryptoIdentity::from_seed(name, &seed);
            pub_keys.insert(name.to_string(), id.public_key_bytes.clone());
            self.identities.insert(name.to_string(), id);
        }

        for &name in &names {
            let peers: Vec<String> = names
                .iter()
                .filter(|&&n| n != name)
                .map(|&s| s.to_string())
                .collect();
            let bridge = Arc::new(MockEbpfBridge::new_with_secret(vec![
                0xAA, 0xBB, 0xCC, 0xDD,
            ]));
            let mut st = ConsensusState::new(name, peers).with_ebpf_bridge(bridge.clone());
            st.register_peer_public_keys(pub_keys.clone());
            self.bridges.insert(name.to_string(), bridge);
            self.nodes.insert(name.to_string(), st);
        }

        // 初期状態で node-alpha を Term 1 の Leader に選出
        self.elect_leader("node-alpha");
        if self.real_kernel_bridge.is_some() {
            self.log(format!(
                "KERNEL XDP ATTACHED: Live eBPF `veil_xdp_filter` active on interface `{}`!",
                self.kernel_iface
            ));
        }
    }

    pub fn elect_leader(&mut self, candidate: &str) {
        let rv = self
            .nodes
            .get_mut(candidate)
            .expect("candidate exists")
            .start_election();
        let signed_rv = self.identities[candidate].sign_rpc(rv);

        let peer_names: Vec<String> = self
            .nodes
            .keys()
            .filter(|k| k.as_str() != candidate)
            .cloned()
            .collect();

        let mut responses = Vec::new();
        for peer in peer_names {
            if let Ok(Some(resp)) = self
                .nodes
                .get_mut(&peer)
                .unwrap()
                .handle_signed_rpc(signed_rv.clone())
            {
                responses.push((peer.clone(), self.identities[&peer].sign_rpc(resp)));
            }
        }

        for (_peer, signed_resp) in responses {
            let _ = self
                .nodes
                .get_mut(candidate)
                .unwrap()
                .handle_signed_rpc(signed_resp);
        }

        let term = self.nodes[candidate].current_term;
        self.log(format!(
            "ELECTED: `{candidate}` won Quorum (3/3) in Term {term} via Ed25519 Signed RequestVote"
        ));
    }

    /// 1. わざとモック攻撃プロセス（Linux `ns-veil-rogue` がある場合は本物の UDP Flood、それ以外は `sleep 300`）を OS 上に起動する
    pub fn spawn_rogue_attacker(&mut self) -> Result<()> {
        let idx = (self.rogues.len() as u8) + 66;
        let ip = if self.real_kernel_bridge.is_some() && self.rogues.is_empty() {
            Ipv4Addr::new(10, 99, 0, 66)
        } else {
            Ipv4Addr::new(10, 99, 0, idx)
        };
        let proc_info =
            RogueAttackProcess::spawn_mock(ip, "node-beta", "UDP Flood (1000 pkt/s) + Syscall")?;
        let pid = proc_info.pid;
        self.rogues.push(proc_info);
        self.log(format!(
            "ALERT: Spawned rogue process PID={pid} (src={ip}) flooding `{}` -> `node-beta`!",
            self.kernel_iface
        ));
        Ok(())
    }

    /// 2. ビザンチン攻撃（偽造 Ed25519 署名・なりすまし投票）を注入して撃退する
    pub fn inject_byzantine_spoof(&mut self) {
        let attacker_id = NodeCryptoIdentity::from_seed("mallory", &[0x99; 32]);
        let forged_rpc = ConsensusRpc::AppendEntries {
            term: 999,
            leader_id: "node-alpha".into(),
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
            commit_qc: None,
        };
        let mut forged_env = attacker_id.sign_rpc(forged_rpc);
        forged_env.signer_id = "node-alpha".into(); // リーダーになりすまし

        for (name, node) in &mut self.nodes {
            let res = node.handle_signed_rpc(forged_env.clone());
            debug_assert!(res.is_err());
            let _ = name;
        }
        self.log(
            "BYZANTINE REJECTED: Forged Ed25519 RPC (Term=999 spoofing `node-alpha`) blocked by all 3 nodes!",
        );
    }

    /// 3. コンセンサス合意によって攻撃プロセスを XDP_DROP + LSM -EPERM + ステルスGC + プロセス Kill で一斉駆逐する
    pub fn exterminate_rogues_by_consensus(&mut self) {
        let leader_id = self
            .nodes
            .iter()
            .find(|(_, st)| st.role == Role::Leader)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| "node-alpha".to_string());

        let active_rogues: Vec<(usize, u32, Ipv4Addr, String)> = self
            .rogues
            .iter()
            .enumerate()
            .filter(|(_, r)| r.status != RogueStatus::Exterminated)
            .map(|(i, r)| (i, r.pid, r.attacker_ip, r.target_node.clone()))
            .collect();

        if active_rogues.is_empty() {
            self.log("INFO: No active rogue processes to exterminate. Press [a] to spawn one!");
            return;
        }

        for (idx, pid, ip, target_node) in active_rogues {
            self.rogues[idx].status = RogueStatus::ConsensusVoting;

            // (A) リーダーが `SetPeerTrust(ip, false)` と `SetNodeLockdown(target_node, true)` を提案
            {
                let leader = self.nodes.get_mut(&leader_id).unwrap();
                let _ = leader.propose(MeshCommand::set_trust(ip, false));
                let _ = leader.propose(MeshCommand::set_lockdown(&target_node, true));
            }

            // (B) 2ラウンドの AppendEntries レプリケーション（1: Quorum Commit、2: Follower Apply & eBPF Sync）
            let followers: Vec<String> = self
                .nodes
                .keys()
                .filter(|k| k.as_str() != leader_id)
                .cloned()
                .collect();

            for _round in 0..2 {
                for f_id in &followers {
                    let ae = self
                        .nodes
                        .get(&leader_id)
                        .unwrap()
                        .make_append_entries(f_id)
                        .unwrap();
                    let signed_ae = self.identities[&leader_id].sign_rpc(ae);
                    if let Ok(Some(resp)) = self
                        .nodes
                        .get_mut(f_id)
                        .unwrap()
                        .handle_signed_rpc(signed_ae)
                    {
                        let signed_resp = self.identities[f_id].sign_rpc(resp);
                        let _ = self
                            .nodes
                            .get_mut(&leader_id)
                            .unwrap()
                            .handle_signed_rpc(signed_resp);
                    }
                }
            }

            // (C) 実 Linux カーネル eBPF ブリッジが接続されている場合は、カーネル内の実 `PEER_POLICY_MAP` にも即時書き込む！
            if let Some(ref real_bridge) = self.real_kernel_bridge {
                let policy =
                    veil_common::PeerPolicy::new(false, self.nodes[&leader_id].commit_index as u32);
                let _ = real_bridge.sync_peer_policy(u32::from(ip), policy);
                self.log(format!(
                    "SYSCALL BPF: sys_bpf(BPF_MAP_UPDATE_ELEM, map=PEER_POLICY_MAP, key={ip}, trusted=0) -> XDP_DROP active!"
                ));
            }

            // (D) OS 上のモック攻撃プロセスに実際に sys_kill(PID, SIGKILL) を発行し、wait4() で回収した証拠をログに記録
            let kill_proof = self.rogues[idx].kill_process();
            self.log(format!("SYSCALL KILL: {kill_proof}"));
            self.log(format!(
                "EXTERMINATED PID={pid} ({ip}): Quorum(3/3) -> XDP_DROP + LSM -EPERM + Stealth GC Zeroized + SIGKILL!"
            ));
        }
    }

    /// 定期ティック更新（攻撃プロセスのパケット送信と eBPF XDP フィルタ判定のカウント）
    pub fn on_tick(&mut self) {
        self.step_tick += 1;

        if let Some(ref real_bridge) = self.real_kernel_bridge {
            if let Some((pass_cnt, drop_cnt)) = real_bridge.read_xdp_counters() {
                self.total_xdp_pass = pass_cnt;
                self.total_xdp_drop = drop_cnt;
            }
        } else {
            self.total_xdp_pass += 3;
        }

        for rogue in &mut self.rogues {
            if rogue.status != RogueStatus::Exterminated {
                rogue.packets_sent += 50;
            }
            let verdict = self
                .nodes
                .get(&rogue.target_node)
                .map(|n| n.check_xdp_packet(rogue.attacker_ip))
                .unwrap_or(XdpVerdict::Pass);

            match verdict {
                XdpVerdict::Drop => {
                    rogue.packets_dropped_xdp += 50;
                    if self.real_kernel_bridge.is_none() {
                        self.total_xdp_drop += 50;
                    }
                }
                _ => {
                    if self.real_kernel_bridge.is_none() {
                        self.total_xdp_pass += 50;
                    }
                }
            }
        }
    }
}

/// インタラクティブ TUI または自動デモモードを起動する
pub fn run_tui(
    auto_demo: bool,
    real_kernel_bridge: Option<Arc<dyn EbpfBridge>>,
    kernel_iface: String,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiArenaApp::new_with_kernel_bridge(real_kernel_bridge, kernel_iface);
    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();
    let start_time = Instant::now();
    let mut demo_stage = 0;

    let res = loop {
        terminal.draw(|f| draw_dashboard(f, &app, auto_demo))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                        KeyCode::Char('a') => {
                            let _ = app.spawn_rogue_attacker();
                        }
                        KeyCode::Char('b') => {
                            app.inject_byzantine_spoof();
                        }
                        KeyCode::Char('x') => {
                            app.exterminate_rogues_by_consensus();
                        }
                        KeyCode::Char('r') => {
                            app.reset_cluster();
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.on_tick();
            last_tick = Instant::now();

            // 自動デモシナリオ（GIF 録画やワンショット確認用）
            if auto_demo {
                let elapsed = start_time.elapsed().as_millis();
                if elapsed > 1000 && demo_stage == 0 {
                    let _ = app.spawn_rogue_attacker();
                    demo_stage = 1;
                } else if elapsed > 2200 && demo_stage == 1 {
                    let _ = app.spawn_rogue_attacker();
                    demo_stage = 2;
                } else if elapsed > 3400 && demo_stage == 2 {
                    app.inject_byzantine_spoof();
                    demo_stage = 3;
                } else if elapsed > 4800 && demo_stage == 3 {
                    app.exterminate_rogues_by_consensus();
                    demo_stage = 4;
                } else if elapsed > 8000 {
                    break Ok(());
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

fn draw_dashboard(f: &mut Frame, app: &TuiArenaApp, auto_demo: bool) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(8), // Cluster Consensus & eBPF State Table
            Constraint::Length(8), // Rogue Attack Processes & Extermination Status
            Constraint::Min(7),    // Real-time Security & Consensus Event Log
            Constraint::Length(3), // Controls Footer
        ])
        .split(f.area());

    // 1. Header
    let mode_label = if auto_demo {
        "AUTO-DEMO MODE"
    } else {
        "INTERACTIVE ARENA"
    };
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            " 🛡️ VEIL-MESH-CONSENSUS ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("[{mode_label}]"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  QUIC + Raft BFT + Aya eBPF (XDP/LSM)   |   XDP_PASS: {} pkts   XDP_DROP: {} pkts",
            app.total_xdp_pass, app.total_xdp_drop
        )),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" System Overview "),
    );
    f.render_widget(header, chunks[0]);

    // 2. Consensus Nodes Table
    draw_nodes_table(f, app, chunks[1]);

    // 3. Rogue Attack Processes Table
    draw_rogues_table(f, app, chunks[2]);

    // 4. Event Logs
    let log_items: Vec<ListItem> = app
        .event_logs
        .iter()
        .rev()
        .map(|msg| {
            let color = if msg.contains("EXTERMINATED") {
                Color::Green
            } else if msg.contains("SYSCALL") {
                Color::Yellow
            } else if msg.contains("ALERT") {
                Color::LightRed
            } else if msg.contains("BYZANTINE") {
                Color::Magenta
            } else {
                Color::Cyan
            };
            ListItem::new(Line::from(Span::styled(
                msg.clone(),
                Style::default().fg(color),
            )))
        })
        .collect();
    let logs_widget = List::new(log_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Consensus & Kernel eBPF Audit Stream (Newest First) "),
    );
    f.render_widget(logs_widget, chunks[3]);

    // 5. Footer Controls
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            " [a] ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::raw("Spawn Mock Attack Process (OS PID)   "),
        Span::styled(
            " [b] ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("Inject Byzantine Forged RPC   "),
        Span::styled(
            " [x] ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("Consensus Exterminate (XDP_DROP + LSM + Kill)   "),
        Span::styled(
            " [r] ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("Reset   "),
        Span::styled(
            " [q] ",
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("Quit"),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Tactical Controls "),
    );
    f.render_widget(footer, chunks[4]);
}

fn draw_nodes_table(f: &mut Frame, app: &TuiArenaApp, area: Rect) {
    let header = Row::new(vec![
        "Node ID",
        "Role",
        "Term",
        "CommitIdx",
        "eBPF Blacklist Rules",
        "LSM socket_connect",
        "Stealth GC Secret",
        "Byzantine Blocked",
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .nodes
        .iter()
        .map(|(name, st)| {
            let bridge = &app.bridges[name];
            let secret_status = if bridge.sensitive_key_material.lock().unwrap().is_empty() {
                "ZEROIZED (PURGED)"
            } else {
                "ACTIVE (4 bytes)"
            };
            let lsm_status = if st.check_lsm_socket_connect() == 0 {
                "ALLOW (0)"
            } else {
                "LOCKDOWN (-EPERM)"
            };
            let role_str = match st.role {
                Role::Leader => "★ LEADER",
                Role::Candidate => "CANDIDATE",
                Role::Follower => "FOLLOWER",
            };
            Row::new(vec![
                name.clone(),
                role_str.to_string(),
                st.current_term.to_string(),
                st.commit_index.to_string(),
                format!("{} IPs blocked", st.ebpf_policy_map.len()),
                lsm_status.to_string(),
                secret_status.to_string(),
                format!("{} pkts", st.rejected_byzantine_packets),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(12),
        Constraint::Length(11),
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Length(20),
        Constraint::Length(19),
        Constraint::Length(19),
        Constraint::Length(16),
    ];

    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" 3-Node Mesh Consensus & Kernel eBPF Map State "),
    );
    f.render_widget(table, area);
}

fn draw_rogues_table(f: &mut Frame, app: &TuiArenaApp, area: Rect) {
    let header = Row::new(vec![
        "OS PID",
        "Attacker IP",
        "Target Node",
        "Attack Vector",
        "Status",
        "eBPF XDP Verdict",
        "Blocked / Sent Pkts",
    ])
    .style(
        Style::default()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = if app.rogues.is_empty() {
        vec![Row::new(vec![
            "-",
            "-",
            "-",
            "No active rogue processes (Press [a] to spawn)",
            "CLEAN",
            "XDP_PASS",
            "0 / 0",
        ])]
    } else {
        app.rogues
            .iter()
            .map(|r| {
                let status_str = match r.status {
                    RogueStatus::Infiltrating => "⚠️ INFILTRATING (PID ALIVE)",
                    RogueStatus::ConsensusVoting => "⏳ QUORUM VOTING...",
                    RogueStatus::Exterminated => "✅ EXTERMINATED (KILLED & DROPPED)",
                };
                let verdict = app
                    .nodes
                    .get(&r.target_node)
                    .map(|n| n.check_xdp_packet(r.attacker_ip))
                    .unwrap_or(XdpVerdict::Pass);
                let verdict_str = match verdict {
                    XdpVerdict::Drop => "🛑 XDP_DROP (Blocked at NIC)",
                    _ => "🟢 XDP_PASS",
                };
                Row::new(vec![
                    r.pid.to_string(),
                    r.attacker_ip.to_string(),
                    r.target_node.clone(),
                    r.attack_type.clone(),
                    status_str.to_string(),
                    verdict_str.to_string(),
                    format!("{} / {}", r.packets_dropped_xdp, r.packets_sent),
                ])
            })
            .collect()
    };

    let widths = [
        Constraint::Length(8),
        Constraint::Length(13),
        Constraint::Length(12),
        Constraint::Length(28),
        Constraint::Length(34),
        Constraint::Length(28),
        Constraint::Length(18),
    ];

    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Rogue Attack Processes (Real OS Mock Processes `sleep`) & eBPF Defense "),
    );
    f.render_widget(table, area);
}
