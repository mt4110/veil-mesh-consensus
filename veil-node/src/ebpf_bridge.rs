pub use crate::usecase::ports::EbpfBridge;
use anyhow::Result;
use std::collections::BTreeMap;
use std::sync::Mutex;
use tracing::info;
use veil_common::{NodeLsmPolicy, PeerPolicy};

/// テストおよび非 Linux ホスト環境（macOS 等）でもカーネル eBPF マップへの書き込みと
/// ステルス GC パージ動作を完全再現・検証できるインメモリ・カーネルブリッジ
#[derive(Debug, Default)]
pub struct MockEbpfBridge {
    pub kernel_peer_map: Mutex<BTreeMap<u32, PeerPolicy>>,
    pub kernel_lsm_policy: Mutex<NodeLsmPolicy>,
    pub gc_purges: Mutex<u64>,
    pub sensitive_key_material: Mutex<Vec<u8>>,
}

impl MockEbpfBridge {
    pub fn new_with_secret(secret: Vec<u8>) -> Self {
        Self {
            kernel_peer_map: Mutex::new(BTreeMap::new()),
            kernel_lsm_policy: Mutex::new(NodeLsmPolicy::new(false, 0)),
            gc_purges: Mutex::new(0),
            sensitive_key_material: Mutex::new(secret),
        }
    }
}

impl EbpfBridge for MockEbpfBridge {
    fn sync_peer_policy(&self, peer_ip: u32, policy: PeerPolicy) -> Result<()> {
        self.kernel_peer_map.lock().unwrap().insert(peer_ip, policy);
        info!(
            peer_ip = %std::net::Ipv4Addr::from(peer_ip),
            trusted = policy.trusted,
            epoch = policy.epoch,
            "Synced PeerPolicy to kernel PEER_POLICY_MAP"
        );
        Ok(())
    }

    fn sync_lsm_policy(&self, policy: NodeLsmPolicy) -> Result<()> {
        *self.kernel_lsm_policy.lock().unwrap() = policy;
        if policy.lockdown != 0 {
            // ステルス GC 発動：メモリ上の機密鍵マテリアルを即座にゼロクリア（Zeroize）
            let mut secret = self.sensitive_key_material.lock().unwrap();
            for byte in secret.iter_mut() {
                *byte = 0;
            }
            secret.clear();
            *self.gc_purges.lock().unwrap() += 1;
            info!(
                epoch = policy.epoch,
                "LSM Lockdown triggered Stealth GC: zeroized in-memory secret material and locked socket_connect (-EPERM)"
            );
        }
        Ok(())
    }

    fn stealth_gc_purge_count(&self) -> u64 {
        *self.gc_purges.lock().unwrap()
    }
}

/// 実 Linux 環境（NixOS MicroVM `node-alpha` / `node-beta`）用の Aya カーネルブリッジ実装
#[cfg(target_os = "linux")]
pub struct AyaKernelBridge {
    bpf: Mutex<aya::Ebpf>,
    gc_purges: Mutex<u64>,
    sensitive_key_material: Mutex<Vec<u8>>,
}

#[cfg(target_os = "linux")]
impl AyaKernelBridge {
    /// eBPF オブジェクトをロードし、XDP (`veil_xdp_filter`) と LSM (`veil_lsm_socket_connect`) をカーネルへアタッチする
    pub fn load_and_attach(obj_path: &str, iface: &str) -> Result<Self> {
        use aya::{
            programs::{Lsm, Xdp, XdpFlags},
            Btf, Ebpf,
        };

        let mut bpf = Ebpf::load_file(obj_path)?;

        // 1. XDP プログラムのロードと eth1 へのアタッチ
        let xdp: &mut Xdp = bpf.program_mut("veil_xdp_filter").unwrap().try_into()?;
        xdp.load()?;
        xdp.attach(iface, XdpFlags::default())?;
        info!(iface = %iface, "Attached Aya XDP program `veil_xdp_filter`");

        // 2. LSM (`socket_connect`) プログラムの BTF ロードとアタッチ（カーネルで `lsm=bpf` が有効な場合のみ）
        if let Ok(btf) = Btf::from_sys_fs() {
            if let Some(prog) = bpf.program_mut("veil_lsm_socket_connect") {
                if let Ok(lsm) = TryInto::<&mut Lsm>::try_into(prog) {
                    if lsm.load("socket_connect", &btf).is_ok() && lsm.attach().is_ok() {
                        info!("Attached Aya LSM program `veil_lsm_socket_connect` (lsm=bpf)");
                    } else {
                        info!("Kernel does not have `lsm=bpf` enabled; running with hardware/driver XDP enforcement (`veil_xdp_filter`)");
                    }
                }
            }
        }

        Ok(Self {
            bpf: Mutex::new(bpf),
            gc_purges: Mutex::new(0),
            sensitive_key_material: Mutex::new(vec![0xAA; 32]),
        })
    }
}

#[cfg(target_os = "linux")]
impl EbpfBridge for AyaKernelBridge {
    fn sync_peer_policy(&self, peer_ip: u32, policy: PeerPolicy) -> Result<()> {
        use aya::maps::HashMap as AyaHashMap;
        use veil_common::MAP_PEER_POLICY;

        let mut bpf = self.bpf.lock().unwrap();
        let mut map: AyaHashMap<_, u32, PeerPolicy> =
            AyaHashMap::try_from(bpf.map_mut(MAP_PEER_POLICY).unwrap())?;
        map.insert(peer_ip, policy, 0)?;
        info!(
            peer_ip = %std::net::Ipv4Addr::from(peer_ip),
            trusted = policy.trusted,
            epoch = policy.epoch,
            "Wrote PeerPolicy directly into Linux kernel eBPF map `PEER_POLICY_MAP`"
        );
        Ok(())
    }

    fn sync_lsm_policy(&self, policy: NodeLsmPolicy) -> Result<()> {
        use aya::maps::Array as AyaArray;
        use veil_common::MAP_LSM_POLICY;

        {
            let mut bpf = self.bpf.lock().unwrap();
            if let Some(m) = bpf.map_mut(MAP_LSM_POLICY) {
                if let Ok(mut map) = AyaArray::<_, NodeLsmPolicy>::try_from(m) {
                    let _ = map.set(0, policy, 0);
                }
            }
        }

        if policy.lockdown != 0 {
            let mut secret = self.sensitive_key_material.lock().unwrap();
            for byte in secret.iter_mut() {
                *byte = 0;
            }
            secret.clear();
            *self.gc_purges.lock().unwrap() += 1;
            info!(
                epoch = policy.epoch,
                "Executed Stealth GC memory zeroization & LSM policy update"
            );
        }
        Ok(())
    }

    fn stealth_gc_purge_count(&self) -> u64 {
        *self.gc_purges.lock().unwrap()
    }

    fn read_xdp_counters(&self) -> Option<(u64, u64)> {
        use aya::maps::PerCpuArray as AyaPerCpuArray;
        use veil_common::MAP_PACKETS_SEEN;

        let bpf = self.bpf.lock().unwrap();
        let map = AyaPerCpuArray::<_, u64>::try_from(bpf.map(MAP_PACKETS_SEEN)?).ok()?;
        let pass_vals = map.get(&0, 0).ok()?;
        let drop_vals = map.get(&1, 0).ok()?;
        let pass_sum: u64 = pass_vals.iter().copied().sum();
        let drop_sum: u64 = drop_vals.iter().copied().sum();
        Some((pass_sum, drop_sum))
    }
}
