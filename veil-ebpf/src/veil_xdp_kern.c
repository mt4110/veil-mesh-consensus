/* Scoped XDP enforcement: exact destination IPv4 + TCP/UDP port,
 * then source-IP policy and finite boot-clock expiry. Plain Ethernet,
 * unfragmented IPv4 only. Other traffic passes outside this profile.
 * Management/source exclusions are validated before map insertion.
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
    __u64 expires_at_boot_ns;
};

/* Legacy map_def format supported by both iproute2 (`ip link set xdp`) and Aya */
struct bpf_map_def {
    __u32 type;
    __u32 key_size;
    __u32 value_size;
    __u32 max_entries;
    __u32 map_flags;
};

struct bpf_map_def SEC("maps") XDP_CONTROL = {
    .type = 2, /* ARRAY: default zero is bypass */
    .key_size = sizeof(__u32),
    .value_size = sizeof(__u32),
    .max_entries = 1,
};

struct service_scope_key { __u32 destination; __u16 protocol; __u16 port; };
struct bpf_map_def SEC("maps") SERVICE_SCOPE = {
    .type = BPF_MAP_TYPE_HASH, .key_size = sizeof(struct service_scope_key),
    .value_size = sizeof(__u8), .max_entries = 16,
};

struct bpf_map_def SEC("maps") PEER_POLICY_MAP = {
    .type = BPF_MAP_TYPE_HASH,
    .key_size = sizeof(__u32),
    .value_size = sizeof(struct peer_policy),
    .max_entries = 4096,
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
static __u64 (*bpf_ktime_get_boot_ns)(void) = (void *)125;

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
    __u32 zero = 0;
    __u32 *enforce = bpf_map_lookup_elem(&XDP_CONTROL, &zero);
    if (!enforce || *enforce != 1) {
        record_packet(0);
        return XDP_PASS;
    }
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

    /* Exact destination/protocol/port; ambiguous fragments and encapsulation
     * are outside this narrow profile. Never broaden to the whole host. */
    if ((ip[0] >> 4) != 4 || (read_be16(ip + 6) & 0x3fff) != 0) return XDP_PASS;
    __u16 port = 0;
    if (ip[9] == 6 || ip[9] == 17) {
        if (data + 14 + ihl + 4 > data_end) return XDP_PASS;
        port = read_be16(ip + ihl + 2);
    }
    struct service_scope_key scope = { .destination = read_be32(ip + 16), .protocol = ip[9], .port = port };
    if (!bpf_map_lookup_elem(&SERVICE_SCOPE, &scope)) return XDP_PASS;

    /* Extract Source IPv4 Address in Host order (matching u32::from(Ipv4Addr)) */
    __u32 src_ip = read_be32(ip + 12);

    struct peer_policy *policy = bpf_map_lookup_elem(&PEER_POLICY_MAP, &src_ip);
    if (policy && policy->trusted == 0 &&
        (policy->expires_at_boot_ns == 0 || bpf_ktime_get_boot_ns() < policy->expires_at_boot_ns)) {
        record_packet(1); /* index 1 = XDP_DROP count */
        return XDP_DROP;
    }

    record_packet(0); /* index 0 = XDP_PASS count */
    return XDP_PASS;
}

char SEC("license") _license[] = "GPL";
