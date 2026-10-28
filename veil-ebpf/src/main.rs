#![no_std]
#![no_main]

use aya_ebpf::{
    bindings::xdp_action,
    macros::{lsm, map, xdp},
    maps::{Array, HashMap, PerCpuArray},
    programs::{LsmContext, XdpContext},
};
use core::mem;
use veil_common::{
    evaluate_lsm_policy, evaluate_peer_policy_at, NodeLsmPolicy, PeerPolicy, ServiceScopeKey,
    XdpVerdict,
};

/// IPv4 アドレス (Host Byte Order `u32`) をキーとするピア信頼ポリシーマップ
#[map]
static PEER_POLICY_MAP: HashMap<u32, PeerPolicy> = HashMap::with_max_entries(4096, 0);

/// LSM ステルス GC / 隔離ロックダウン制御マップ (`index 0`: 自ノードの `NodeLsmPolicy`)
#[map]
static LSM_POLICY_MAP: Array<NodeLsmPolicy> = Array::with_max_entries(1, 0);

/// XDP で処理したパケット数のカウンタ (`index 0`: PASS, `index 1`: DROP)
#[map]
static PACKETS_SEEN_MAP: PerCpuArray<u64> = PerCpuArray::with_max_entries(2, 0);

#[map]
static XDP_CONTROL: Array<u32> = Array::with_max_entries(1, 0);

#[map]
static SERVICE_SCOPE: HashMap<ServiceScopeKey, u8> = HashMap::with_max_entries(16, 0);

const ETH_P_IP: u16 = 0x0800;

#[repr(C)]
struct EthHdr {
    h_dest: [u8; 6],
    h_source: [u8; 6],
    h_proto: u16,
}

#[repr(C)]
struct Ipv4Hdr {
    version_ihl: u8,
    tos: u8,
    tot_len: u16,
    id: u16,
    frag_off: u16,
    ttl: u8,
    protocol: u8,
    check: u16,
    saddr: u32,
    daddr: u32,
}

#[inline(always)]
fn ptr_at<T>(ctx: &XdpContext, offset: usize) -> Result<*const T, ()> {
    let start = ctx.data();
    let end = ctx.data_end();
    let len = mem::size_of::<T>();

    if start + offset + len > end {
        return Err(());
    }

    Ok((start + offset) as *const T)
}

#[xdp]
pub fn veil_xdp_filter(ctx: XdpContext) -> u32 {
    match try_veil_xdp_filter(ctx) {
        Ok(ret) => ret,
        Err(_) => xdp_action::XDP_PASS,
    }
}

fn try_veil_xdp_filter(ctx: XdpContext) -> Result<u32, ()> {
    if XDP_CONTROL.get(0).copied() != Some(1) {
        if let Some(pass_cnt) = PACKETS_SEEN_MAP.get_ptr_mut(0) {
            unsafe { *pass_cnt += 1 };
        }
        return Ok(xdp_action::XDP_PASS);
    }
    let eth = unsafe { core::ptr::read_unaligned(ptr_at::<EthHdr>(&ctx, 0)?) };
    if u16::from_be(eth.h_proto) != ETH_P_IP {
        return Ok(xdp_action::XDP_PASS);
    }

    // Ethernet is 14 bytes; the IPv4 header need not be aligned for Ipv4Hdr.
    let ipv4 =
        unsafe { core::ptr::read_unaligned(ptr_at::<Ipv4Hdr>(&ctx, mem::size_of::<EthHdr>())?) };
    let ihl = usize::from(ipv4.version_ihl & 0x0f) * 4;
    if ihl < 20 || ctx.data() + mem::size_of::<EthHdr>() + ihl > ctx.data_end() {
        return Ok(xdp_action::XDP_PASS);
    }
    if ipv4.version_ihl >> 4 != 4 || u16::from_be(ipv4.frag_off) & 0x3fff != 0 {
        return Ok(xdp_action::XDP_PASS);
    }
    let port = if ipv4.protocol == 6 || ipv4.protocol == 17 {
        let p = ptr_at::<u16>(&ctx, mem::size_of::<EthHdr>() + ihl + 2)?;
        u16::from_be(unsafe { core::ptr::read_unaligned(p) })
    } else {
        0
    };
    let scope = ServiceScopeKey {
        destination: u32::from_be(ipv4.daddr),
        protocol: u16::from(ipv4.protocol),
        port,
    };
    if unsafe { SERVICE_SCOPE.get(&scope) }.is_none() {
        return Ok(xdp_action::XDP_PASS);
    }
    let src_ip = u32::from_be(ipv4.saddr);

    // マップにポリシーが登録されており、trusted == 0 (Untrusted) の場合は XDP_DROP
    let policy = unsafe { PEER_POLICY_MAP.get(&src_ip) };
    if evaluate_peer_policy_at(policy, unsafe {
        aya_ebpf::helpers::bpf_ktime_get_boot_ns()
    }) == XdpVerdict::Drop
    {
        if let Some(drop_cnt) = PACKETS_SEEN_MAP.get_ptr_mut(1) {
            unsafe { *drop_cnt += 1 };
        }
        return Ok(xdp_action::XDP_DROP);
    }

    if let Some(pass_cnt) = PACKETS_SEEN_MAP.get_ptr_mut(0) {
        unsafe { *pass_cnt += 1 };
    }
    Ok(xdp_action::XDP_PASS)
}

/// LSM フック (`socket_connect`) によるコンセンサス連動型ステルス GC・自己隔離ガード
/// `LSM_POLICY_MAP[0].lockdown != 0` の場合、カーネルレベルで `-EPERM (-1)` を返して通信を遮断する
#[lsm(hook = "socket_connect")]
pub fn veil_lsm_socket_connect(_ctx: LsmContext) -> i32 {
    let policy = LSM_POLICY_MAP.get(0);
    evaluate_lsm_policy(policy)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
