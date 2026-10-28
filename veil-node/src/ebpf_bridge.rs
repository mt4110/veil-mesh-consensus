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
    pub xdp_bypass: Mutex<bool>,
    pub gc_purges: Mutex<u64>,
    pub sensitive_key_material: Mutex<Vec<u8>>,
}

impl MockEbpfBridge {
    pub fn new_with_secret(secret: Vec<u8>) -> Self {
        Self {
            kernel_peer_map: Mutex::new(BTreeMap::new()),
            kernel_lsm_policy: Mutex::new(NodeLsmPolicy::new(false, 0)),
            xdp_bypass: Mutex::new(false),
            gc_purges: Mutex::new(0),
            sensitive_key_material: Mutex::new(secret),
        }
    }
}

impl EbpfBridge for MockEbpfBridge {
    fn set_xdp_bypass(&self, enabled: bool) -> Result<()> {
        *self.xdp_bypass.lock().unwrap() = enabled;
        Ok(())
    }

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
    lsm_attached: bool,
    gc_purges: Mutex<u64>,
    sensitive_key_material: Mutex<Vec<u8>>,
}

#[cfg(target_os = "linux")]
impl AyaKernelBridge {
    /// eBPF オブジェクトをロードし、XDP (`veil_xdp_filter`) と LSM (`veil_lsm_socket_connect`) をカーネルへアタッチする
    pub fn load_and_attach(obj_path: &str, iface: &str) -> Result<Self> {
        // Retained isolated legacy regression fixture; production requires the
        // configuration-bound TCP/UDP scope passed by normal daemon startup.
        #[cfg(test)]
        {
            Self::load_and_attach_with_scopes(
                obj_path,
                iface,
                &[crate::domain::ServiceScope {
                    destination: u32::from(std::net::Ipv4Addr::new(10, 99, 0, 1)),
                    protocol: 1,
                    port: 0,
                }],
            )
        }
        #[cfg(not(test))]
        {
            let _ = (obj_path, iface);
            anyhow::bail!("Explicit authorized service scope required; use normal daemon")
        }
    }
    pub fn load_and_attach_with_scopes(
        obj_path: &str,
        iface: &str,
        scopes: &[crate::domain::ServiceScope],
    ) -> Result<Self> {
        use aya::{
            programs::{Lsm, Xdp, XdpFlags},
            Btf, Ebpf,
        };

        // FD-owned XDP links ensure process death releases this program. Avoid
        // older netlink attachment semantics, which can survive an unclean exit.
        anyhow::ensure!(
            aya::util::KernelVersion::current()? >= aya::util::KernelVersion::new(5, 9, 0),
            "XDP enforcement requires Linux >= 5.9 with FD-owned links"
        );
        let mut bpf = Ebpf::load_file(obj_path)?;

        // Validate both map ABIs before attachment. Older objects are incompatible.
        let _ = aya::maps::HashMap::<_, u32, PeerPolicy>::try_from(
            bpf.map(veil_common::MAP_PEER_POLICY)
                .ok_or_else(|| anyhow::anyhow!("Missing peer map"))?,
        )?;
        anyhow::ensure!(
            !scopes.is_empty() && scopes.len() <= 16,
            "Explicit service scope required"
        );
        let mut map = aya::maps::HashMap::<_, veil_common::ServiceScopeKey, u8>::try_from(
            bpf.map_mut(veil_common::MAP_SERVICE_SCOPE)
                .ok_or_else(|| anyhow::anyhow!("Missing service-scope ABI; old object refused"))?,
        )?;
        for s in scopes {
            map.insert(
                veil_common::ServiceScopeKey {
                    destination: s.destination,
                    protocol: u16::from(s.protocol),
                    port: s.port,
                },
                1,
                0,
            )?;
        }
        let mut control = aya::maps::Array::<_, u32>::try_from(
            bpf.map_mut(veil_common::MAP_XDP_CONTROL)
                .ok_or_else(|| anyhow::anyhow!("Missing XDP control map; rebuild object"))?,
        )?;
        control.set(0, 1, 0)?;

        // 1. XDP プログラムのロードと eth1 へのアタッチ
        let xdp: &mut Xdp = bpf
            .program_mut("veil_xdp_filter")
            .ok_or_else(|| anyhow::anyhow!("Missing veil_xdp_filter program"))?
            .try_into()?;
        xdp.load()?;
        xdp.attach(iface, XdpFlags::default())?;
        info!(iface = %iface, "Attached Aya XDP program `veil_xdp_filter`");

        // 2. LSM (`socket_connect`) プログラムの BTF ロードとアタッチ（カーネルで `lsm=bpf` が有効な場合のみ）
        let mut lsm_attached = false;
        if let Ok(btf) = Btf::from_sys_fs() {
            if let Some(prog) = bpf.program_mut("veil_lsm_socket_connect") {
                if let Ok(lsm) = TryInto::<&mut Lsm>::try_into(prog) {
                    if lsm.load("socket_connect", &btf).is_ok() && lsm.attach().is_ok() {
                        lsm_attached = true;
                        info!("Attached Aya LSM program `veil_lsm_socket_connect` (lsm=bpf)");
                    } else {
                        info!("Kernel does not have `lsm=bpf` enabled; running with hardware/driver XDP enforcement (`veil_xdp_filter`)");
                    }
                }
            }
        }

        Ok(Self {
            bpf: Mutex::new(bpf),
            lsm_attached,
            gc_purges: Mutex::new(0),
            sensitive_key_material: Mutex::new(vec![0xAA; 32]),
        })
    }
}

#[cfg(target_os = "linux")]
impl EbpfBridge for AyaKernelBridge {
    fn set_xdp_bypass(&self, enabled: bool) -> Result<()> {
        let mut bpf = self.bpf.lock().unwrap();
        let mut control = aya::maps::Array::<_, u32>::try_from(
            bpf.map_mut(veil_common::MAP_XDP_CONTROL)
                .ok_or_else(|| anyhow::anyhow!("Missing XDP control map"))?,
        )?;
        control.set(0, u32::from(!enabled), 0)?;
        Ok(())
    }

    fn sync_peer_policy(&self, peer_ip: u32, policy: PeerPolicy) -> Result<()> {
        use aya::maps::HashMap as AyaHashMap;
        use veil_common::MAP_PEER_POLICY;

        let mut bpf = self.bpf.lock().unwrap();
        let mut map: AyaHashMap<_, u32, PeerPolicy> = AyaHashMap::try_from(
            bpf.map_mut(MAP_PEER_POLICY)
                .ok_or_else(|| anyhow::anyhow!("Missing peer policy map"))?,
        )?;
        if policy.trusted != 0 {
            // R6 対策: 遮断解除時はマップからエントリを削除してカーネル eBPF マップ容量を回収する
            match map.remove(&peer_ip) {
                Ok(()) => {}
                Err(aya::maps::MapError::SyscallError(err))
                    if err.io_error.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        } else {
            map.insert(peer_ip, policy, 0)?;
        }
        info!(
            peer_ip = %std::net::Ipv4Addr::from(peer_ip),
            trusted = policy.trusted,
            epoch = policy.epoch,
            "Synced PeerPolicy directly with Linux kernel eBPF map `PEER_POLICY_MAP`"
        );
        Ok(())
    }

    fn sync_lsm_policy(&self, policy: NodeLsmPolicy) -> Result<()> {
        use aya::maps::Array as AyaArray;
        use veil_common::MAP_LSM_POLICY;

        anyhow::ensure!(
            self.lsm_attached,
            "LSM enforcement is unavailable: program is not attached"
        );
        {
            let mut bpf = self.bpf.lock().unwrap();
            let map = bpf
                .map_mut(MAP_LSM_POLICY)
                .ok_or_else(|| anyhow::anyhow!("Missing LSM policy map"))?;
            let mut map = AyaArray::<_, NodeLsmPolicy>::try_from(map)?;
            map.set(0, policy, 0)?;
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
