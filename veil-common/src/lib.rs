#![no_std]

/// コンセンサスによって共有されるピアの信頼状態
/// - `0`: Blocked / Untrusted (`XDP_DROP`)
/// - `1`: Trusted (`XDP_PASS`)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerPolicy {
    pub trusted: u8,
    pub _pad: [u8; 3],
    pub epoch: u32,
    /// Absolute CLOCK_BOOTTIME deadline in ns; zero means no expiry.
    pub expires_at_boot_ns: u64,
}

impl PeerPolicy {
    pub const fn new(trusted: bool, epoch: u32) -> Self {
        Self {
            trusted: if trusted { 1 } else { 0 },
            _pad: [0; 3],
            epoch,
            expires_at_boot_ns: 0,
        }
    }
}

/// LSM (`lsm=bpf`) ステルス GC / 自己保護用のノード隔離ポリシー
/// - `lockdown == 0`: Normal（外向きソケット接続・操作を許可 `0`）
/// - `lockdown == 1`: Quarantine / Stealth GC Mode（不正プロセスや隔離状態でのソケット接続等を `-EPERM (-1)` で拒否）
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeLsmPolicy {
    pub lockdown: u8,
    pub _pad: [u8; 3],
    pub epoch: u32,
}

impl NodeLsmPolicy {
    pub const fn new(lockdown: bool, epoch: u32) -> Self {
        Self {
            lockdown: if lockdown { 1 } else { 0 },
            _pad: [0; 3],
            epoch,
        }
    }
}

/// XDP パケットフィルタの判定結果（カーネル空間・ユーザー空間で共通のアルゴリズムを実行）
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XdpVerdict {
    Aborted = 0,
    Drop = 1,
    Pass = 2,
    Tx = 3,
    Redirect = 4,
}

/// eBPF XDP 層と User 空間シミュレータの双方で使用する純粋なパケット判定ロジック
#[inline(always)]
pub fn evaluate_peer_policy(policy: Option<&PeerPolicy>) -> XdpVerdict {
    match policy {
        Some(p) if p.trusted == 0 => XdpVerdict::Drop,
        _ => XdpVerdict::Pass,
    }
}

/// Kernel and simulator must both use the same boot clock for leases.
#[inline(always)]
pub fn evaluate_peer_policy_at(policy: Option<&PeerPolicy>, now_ns: u64) -> XdpVerdict {
    match policy {
        Some(p) if p.expires_at_boot_ns != 0 && now_ns >= p.expires_at_boot_ns => XdpVerdict::Pass,
        _ => evaluate_peer_policy(policy),
    }
}

/// eBPF LSM (`socket_connect` 等) と User 空間の双方で使用するロックダウン判定ロジック
/// - `0`: 許可 (Allow)
/// - `-1` (`-EPERM`): 操作拒否 (Operation not permitted)
#[inline(always)]
pub fn evaluate_lsm_policy(policy: Option<&NodeLsmPolicy>) -> i32 {
    match policy {
        Some(p) if p.lockdown != 0 => -1, // -EPERM
        _ => 0,
    }
}

/// eBPF マップ名定数
/// Zero (including a fresh map) bypasses XDP; one enables enforcement.
pub const MAP_XDP_CONTROL: &str = "XDP_CONTROL";
pub const MAP_PEER_POLICY: &str = "PEER_POLICY_MAP";
pub const MAP_PACKETS_SEEN: &str = "PACKETS_SEEN_MAP";
pub const MAP_LSM_POLICY: &str = "LSM_POLICY_MAP";

#[cfg(feature = "user")]
unsafe impl aya::Pod for PeerPolicy {}

#[cfg(feature = "user")]
unsafe impl aya::Pod for NodeLsmPolicy {}

/// Exact service tuple. Host order, explicit 8-byte ABI without padding.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceScopeKey {
    pub destination: u32,
    pub protocol: u16,
    pub port: u16,
}
pub const MAP_SERVICE_SCOPE: &str = "SERVICE_SCOPE";
#[cfg(feature = "user")]
unsafe impl aya::Pod for ServiceScopeKey {}
