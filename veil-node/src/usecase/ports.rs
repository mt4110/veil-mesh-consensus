use anyhow::Result;
use veil_common::{NodeLsmPolicy, PeerPolicy};

/// 【UseCase Output Port】: 合意形成されたポリシーをカーネル空間（またはテスト用モック）の eBPF マップへ同期するための抽象インターフェース
pub trait EbpfBridge: Send + Sync {
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
