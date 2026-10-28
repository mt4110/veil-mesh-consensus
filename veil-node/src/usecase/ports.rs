use anyhow::Result;
use veil_common::{NodeLsmPolicy, PeerPolicy};

/// Atomically replace and durably sync a consensus snapshot. An error means
/// the caller must stop issuing votes until the node is restarted/recovered.
pub trait ConsensusPersistence: Send + Sync {
    fn persist(&self, snapshot: &[u8]) -> std::io::Result<()>;
}

/// 【UseCase Output Port】: 合意形成されたポリシーをカーネル空間（またはテスト用モック）の eBPF マップへ同期するための抽象インターフェース
pub trait EbpfBridge: Send + Sync {
    /// Unsupported backends must not acknowledge an emergency control change.
    fn set_xdp_bypass(&self, _enabled: bool) -> Result<()> {
        anyhow::bail!("XDP bypass control is unavailable")
    }

    /// `PEER_POLICY_MAP` の特定 IP の信頼ポリシー（`XDP_PASS` / `XDP_DROP`）を更新する
    fn sync_peer_policy(&self, peer_ip: u32, policy: PeerPolicy) -> Result<()>;

    /// `LSM_POLICY_MAP[0]` のノード隔離・ステルス GC ポリシー（`0` / `-EPERM`）を更新する
    fn sync_lsm_policy(&self, policy: NodeLsmPolicy) -> Result<()>;

    /// ステルス GC 発動時に機密メモリやエフェメラル状態のゼロ消去（パージ）を実行した回数を返す
    fn stealth_gc_purge_count(&self) -> u64;

    /// カーネルまたはシミュレータの `PACKETS_SEEN_MAP` から `(XDP_PASS, XDP_DROP)` の実パケット数を取得する
    fn read_xdp_counters(&self) -> Option<(u64, u64)> {
        None
    }
}

/// Time source is bound once, before loading state. Boot IDs delimit clock epochs.
pub trait PolicyClock: Send + Sync {
    fn boot_id(&self) -> &str;
    fn now_ns(&self) -> std::io::Result<u64>;
    fn lease_time(&self) -> std::io::Result<LeaseTime> {
        Err(std::io::Error::other(
            "Clock has no verified wall-time uncertainty bound",
        ))
    }
}

/// A bounded wall-time observation paired with an earlier boot-clock sample.
/// The source must bound clock lag, including sampling skew, and the maximum
/// slow rate during the lease. These are trust assumptions, not BFT evidence.
#[derive(Clone, Copy, Debug)]
pub struct LeaseTime {
    pub boot_ns: u64,
    pub unix_ns: u64,
    pub uncertainty_ns: u64,
    pub max_slow_rate_ppm: u32,
}

impl LeaseTime {
    pub fn upper_unix_ns(self) -> std::io::Result<u64> {
        self.unix_ns
            .checked_add(self.uncertainty_ns)
            .ok_or_else(|| std::io::Error::other("Wall clock bound overflow"))
    }

    pub fn boot_deadline(self, issued_at: u64, not_after: u64) -> std::io::Result<u64> {
        let upper = self.upper_unix_ns()?;
        if self.max_slow_rate_ppm >= 1_000_000 {
            return Err(std::io::Error::other("Invalid clock rate bound"));
        }
        // Never schedule future-dated decisions for later activation. Value 1
        // is an expired tombstone; 0 is reserved for indefinite kernel rules.
        if issued_at > upper || not_after <= upper {
            return Ok(1);
        }
        let remaining = not_after - upper;
        let bounded = (u128::from(remaining) * u128::from(1_000_000 - self.max_slow_rate_ppm)
            / 1_000_000) as u64;
        if bounded == 0 {
            return Ok(1);
        }
        self.boot_ns
            .checked_add(bounded)
            .ok_or_else(|| std::io::Error::other("Boot deadline overflow"))
    }
}
