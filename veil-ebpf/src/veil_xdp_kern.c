/*
 * veil-mesh-consensus: Real Linux Kernel XDP Filter for pi-node (`enp0s31f6` / `veth`)
 *
 * Safety Features:
 * 1. Always XDP_PASS non-IPv4 traffic (ARP, IPv6, etc.)
 * 2. Always XDP_PASS SSH port 10023 & 22 so remote management is NEVER severed
 * 3. Look up source IPv4 in `PEER_POLICY_MAP` (or destination UDP port == 9999 when policy is Untrusted):
 *    - If policy.trusted == 0 -> increment PACKETS_SEEN_MAP[1] (DROP) and return XDP_DROP (1)
 *    - Otherwise -> increment PACKETS_SEEN_MAP[0] (PASS) and return XDP_PASS (2)
 */

typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;

#define SEC(name) __attribute__((section(name), used))
#define __always_inline inline __attribute__((always_inline))

#define BPF_MAP_TYPE_HASH 1
#define BPF_MAP_TYPE_PERCPU_ARRAY 6
#define BPF_FUNC_map_lookup_elem 1

#define XDP_ABORTED 0
#define XDP_DROP 1
#define XDP_PASS 2

struct xdp_md {
    __u32 data;
    __u32 data_end;
    __u32 data_meta;
    __u32 ingress_ifindex;
    __u32 rx_queue_index;
    __u32 egress_ifindex;
};

struct peer_policy {
    __u8 trusted;
    __u8 _pad[3];
    __u32 epoch;
};

/* Legacy map_def format supported by both iproute2 (`ip link set xdp`) and Aya */
struct bpf_map_def {
    __u32 type;
    __u32 key_size;
    __u32 value_size;
    __u32 max_entries;
    __u32 map_flags;
};

struct bpf_map_def SEC("maps") PEER_POLICY_MAP = {
    .type = BPF_MAP_TYPE_HASH,
    .key_size = sizeof(__u32),
    .value_size = sizeof(struct peer_policy),
    .max_entries = 256,
    .map_flags = 0,
};

struct bpf_map_def SEC("maps") PACKETS_SEEN_MAP = {
    .type = BPF_MAP_TYPE_PERCPU_ARRAY,
    .key_size = sizeof(__u32),
    .value_size = sizeof(__u64),
    .max_entries = 2,
    .map_flags = 0,
};

static void *(*bpf_map_lookup_elem)(void *map, const void *key) = (void *)BPF_FUNC_map_lookup_elem;

static __always_inline __u16 read_be16(const __u8 *p) {
    return ((__u16)p[0] << 8) | (__u16)p[1];
}

static __always_inline __u32 read_be32(const __u8 *p) {
    return ((__u32)p[0] << 24) | ((__u32)p[1] << 16) | ((__u32)p[2] << 8) | (__u32)p[3];
}

static __always_inline void record_packet(__u32 idx) {
    __u64 *cnt = bpf_map_lookup_elem(&PACKETS_SEEN_MAP, &idx);
    if (cnt) {
        *cnt += 1;
    }
}

SEC("xdp")
int veil_xdp_filter(struct xdp_md *ctx) {
    const __u8 *data = (const __u8 *)(unsigned long)ctx->data;
    const __u8 *data_end = (const __u8 *)(unsigned long)ctx->data_end;

    /* Ethernet (14) + IPv4 (20) = 34 bytes minimum */
    if (data + 34 > data_end) {
        return XDP_PASS;
    }

    /* Check EtherType == 0x0800 (IPv4) */
    if (read_be16(data + 12) != 0x0800) {
        return XDP_PASS;
    }

    const __u8 *ip = data + 14;
    __u8 ihl = (ip[0] & 0x0F) * 4;
    if (ihl < 20 || data + 14 + ihl > data_end) {
        return XDP_PASS;
    }

    __u8 proto = ip[9];

    /* SAFETY GUARD: Never drop SSH (TCP port 10023 or 22) on pi-node! */
    if (proto == 6 && data + 14 + ihl + 4 <= data_end) {
        const __u8 *tcp = data + 14 + ihl;
        __u16 sport = read_be16(tcp);
        __u16 dport = read_be16(tcp + 2);
        if (sport == 10023 || dport == 10023 || sport == 22 || dport == 22) {
            return XDP_PASS;
        }
    }

    /* Extract Source IPv4 Address in Host order (matching u32::from(Ipv4Addr)) */
    __u32 src_ip = read_be32(ip + 12);

    struct peer_policy *policy = bpf_map_lookup_elem(&PEER_POLICY_MAP, &src_ip);
    if (policy && policy->trusted == 0) {
        record_packet(1); /* index 1 = XDP_DROP count */
        return XDP_DROP;
    }

    record_packet(0); /* index 0 = XDP_PASS count */
    return XDP_PASS;
}

char SEC("license") _license[] = "GPL";
