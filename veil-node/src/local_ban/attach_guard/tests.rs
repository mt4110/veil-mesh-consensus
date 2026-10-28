use super::*;

fn names(list: &[&str]) -> Vec<u8> {
    list.iter()
        .flat_map(|s| {
            let mut v = vec![0u8; NAME_LEN];
            v[..s.len()].copy_from_slice(s.as_bytes());
            v
        })
        .collect()
}
fn off() -> Features {
    Features::decode(&names(&["rx-lro", "rx-gro-hw", "rx-gro"]), &[0, 0, 0, 0], 3).unwrap()
}
fn attr(kind: u16, value: &[u8]) -> Vec<u8> {
    let len = value.len() + 4;
    let mut out = vec![0; (len + 3) & !3];
    out[..2].copy_from_slice(&(len as u16).to_ne_bytes());
    out[2..4].copy_from_slice(&kind.to_ne_bytes());
    out[4..len].copy_from_slice(value);
    out
}
fn link() -> Vec<u8> {
    let mut b = vec![0u8; 16];
    b[2..4].copy_from_slice(&1u16.to_ne_bytes());
    b[4..8].copy_from_slice(&7u32.to_ne_bytes());
    b.extend(attr(3, b"test0\0"));
    b.extend(attr(4, &1460u32.to_ne_bytes()));
    b.extend(attr(43, &attr(2, &[0])));
    b
}
fn guard() -> Guard {
    Guard {
        before: LinkState::decode(&link()).unwrap(),
        features: off(),
        qdiscs: vec![],
    }
}
fn installed() -> LinkState {
    let mut l = LinkState::decode(&link()).unwrap();
    l.xdp_mode = 2;
    l.programs = [42, 0, 42, 0];
    l
}

#[test]
fn modes_are_exact_and_no_fallback_flag_exists() {
    assert_eq!(AttachMode::Default.flags().bits(), 0);
    assert_eq!(AttachMode::Generic.flags().bits(), 2);
    assert_eq!(AttachMode::value_variants().len(), 2);
    assert!(AttachMode::from_str("auto", false).is_err());
    assert!(AttachMode::from_str("generic", false).is_ok());
}
#[test]
fn off_active_and_wanted_admitted() {
    off().admit().unwrap();
}
#[test]
fn either_active_or_wanted_on_is_rejected_for_both_features() {
    for bit in 0..2 {
        for wanted in [false, true] {
            let mut f = off();
            if wanted {
                f.wanted[0] |= 1 << bit;
            } else {
                f.active[0] |= 1 << bit;
            }
            assert!(f.admit().is_err());
        }
    }
}
#[test]
fn unknown_missing_and_duplicate_features_refused() {
    assert!(Features::decode(&names(&["rx-lro", "other"]), &[0; 4], 2)
        .unwrap()
        .admit()
        .is_err());
    assert!(Features::decode(&names(&["rx-lro", "rx-lro"]), &[0; 4], 2).is_err());
    assert!(Features::decode(&[], &[], 0).is_err());
    assert!(Features::decode(&[], &[], MAX_FEATURES + 1).is_err());
}
#[test]
fn feature_payload_bounds_padding_and_word_boundary_checked() {
    assert!(Features::decode(&names(&["rx-lro"]), &[], 1).is_err());
    let mut bad = names(&["rx-lro"]);
    bad[31] = b'x';
    assert!(Features::decode(&bad, &[0; 4], 1).is_err());
    assert!(Features::decode(&[b'x'; 32], &[0; 4], 1).is_err());
    let mut list = vec![""; 64];
    list[31] = "rx-lro";
    list[32] = "rx-gro-hw";
    let mut blocks = [0u32; 8];
    let n = names(&list);
    Features::decode(&n, &blocks, 64).unwrap().admit().unwrap();
    blocks[5] = 1;
    assert!(Features::decode(&n, &blocks, 64).unwrap().admit().is_err());
    blocks[5] = 0;
    blocks[2] = 1 << 31;
    assert!(Features::decode(&n, &blocks, 64).unwrap().admit().is_err());
}
#[test]
fn generic_postflight_requires_exact_program_and_mode() {
    let g = guard();
    let good = installed();
    assert_eq!(
        g.validate_after(&good, &off(), &[], AttachMode::Generic, 42)
            .unwrap(),
        "generic"
    );
    assert_eq!(
        g.validate_after(&good, &off(), &[], AttachMode::Default, 42)
            .unwrap(),
        "generic"
    );
    for mode in [0, 1, 3, 4] {
        let mut l = installed();
        l.xdp_mode = mode;
        assert!(g
            .validate_after(&l, &off(), &[], AttachMode::Generic, 42)
            .is_err());
    }
    for ids in [[41, 0, 41, 0], [42, 42, 42, 0], [42, 0, 42, 42], [0; 4]] {
        let mut l = installed();
        l.programs = ids;
        assert!(g
            .validate_after(&l, &off(), &[], AttachMode::Generic, 42)
            .is_err());
    }
    assert!(g
        .validate_after(&good, &off(), &[], AttachMode::Generic, 0)
        .is_err());
    let mut native = installed();
    native.xdp_mode = 1;
    native.programs = [42, 42, 0, 0];
    assert_eq!(
        g.validate_after(&native, &off(), &[], AttachMode::Default, 42)
            .unwrap(),
        "driver"
    );
    assert!(g
        .validate_after(&native, &off(), &[], AttachMode::Generic, 42)
        .is_err());
}
#[test]
fn any_active_wanted_or_name_drift_refuses_ready() {
    let g = guard();
    for i in 0..3 {
        let mut f = off();
        match i {
            0 => f.active[0] = 4,
            1 => f.wanted[0] = 4,
            _ => f.names[2] = "different".into(),
        }
        assert!(g
            .validate_after(&installed(), &f, &[], AttachMode::Generic, 42)
            .is_err());
    }
}
#[test]
fn identity_mtu_topology_or_qdisc_drift_refuses_ready() {
    let g = guard();
    for i in 0..4 {
        let mut l = installed();
        match i {
            0 => l.index += 1,
            1 => l.name = "other".into(),
            2 => l.mtu += 1,
            _ => l.kind = Some("veth".into()),
        }
        assert!(g
            .validate_after(&l, &off(), &[], AttachMode::Generic, 42)
            .is_err());
    }
    assert!(g
        .validate_after(&installed(), &off(), &[vec![1]], AttachMode::Generic, 42)
        .is_err());
}
#[test]
fn existing_xdp_in_any_mode_refused() {
    for mode in 1..=4 {
        let mut l = LinkState::decode(&link()).unwrap();
        l.xdp_mode = mode;
        assert!(l.vacant().is_err());
    }
    for i in 0..4 {
        let mut l = LinkState::decode(&link()).unwrap();
        l.programs[i] = 1;
        assert!(l.vacant().is_err());
    }
}
#[test]
fn malformed_and_duplicate_attributes_fail_closed() {
    for b in [
        vec![0],
        vec![3, 0, 1, 0],
        vec![9, 0, 1, 0],
        vec![4, 0, 1, 0x40],
    ] {
        assert!(attributes(&b).is_err());
    }
    let mut b = attr(1, &[]);
    b.extend(attr(1, &[]));
    assert!(attributes(&b).is_err());
    let mut b = vec![];
    for i in 0..257 {
        b.extend(attr(i, &[]));
    }
    assert!(attributes(&b).is_err());
}
#[test]
fn stacked_unknown_slave_and_master_devices_rejected() {
    for kind in ["vlan", "bond", "bridge", "macvlan", "unknown"] {
        let mut b = link();
        let mut v = kind.as_bytes().to_vec();
        v.push(0);
        b.extend(attr(18, &attr(1, &v)));
        assert!(LinkState::decode(&b).is_err());
    }
    let mut b = link();
    b.extend(attr(10, &1u32.to_ne_bytes()));
    assert!(LinkState::decode(&b).is_err());
    let mut b = link();
    b.extend(attr(5, &8u32.to_ne_bytes()));
    assert!(LinkState::decode(&b).is_err());
    let mut b = link();
    b.extend(attr(18, &attr(4, b"slave\0")));
    assert!(LinkState::decode(&b).is_err());
}
#[test]
fn isolated_veth_peer_is_not_a_lower_device() {
    let mut b = link();
    b.extend(attr(18, &attr(1, b"veth\0")));
    b.extend(attr(5, &8u32.to_ne_bytes()));
    LinkState::decode(&b).unwrap().vacant().unwrap();
}
#[test]
fn missing_or_invalid_link_state_refused() {
    let mut b = link();
    b[2] = 0;
    assert!(LinkState::decode(&b).is_err());
    let b = link();
    assert!(LinkState::decode(&b[..b.len() - 12]).is_err());
    assert!(LinkState::decode(&[]).is_err());
}
fn qdisc(kind: &[u8], index: u32) -> Vec<u8> {
    let mut b = vec![0u8; 20];
    b[4..8].copy_from_slice(&index.to_ne_bytes());
    b.extend(attr(1, kind));
    b
}
#[test]
fn tc_hooks_and_unknown_qdiscs_refused_but_other_interfaces_untouched() {
    for kind in [b"clsact\0".as_slice(), b"ingress\0", b"htb\0", b"unknown\0"] {
        assert!(decode_qdiscs(vec![qdisc(kind, 7)], 7).is_err());
        assert!(decode_qdiscs(vec![qdisc(kind, 8)], 7).unwrap().is_empty());
    }
    for kind in [
        b"noqueue\0".as_slice(),
        b"mq\0",
        b"fq_codel\0",
        b"pfifo_fast\0",
        b"fq\0",
    ] {
        assert_eq!(decode_qdiscs(vec![qdisc(kind, 7)], 7).unwrap().len(), 1);
    }
    assert!(decode_qdiscs(vec![vec![0; 19]], 7).is_err());
    assert!(decode_qdiscs(vec![qdisc(b"noqueue\0", 7); 17], 7).is_err());
    let mut invalid_handle = qdisc(b"fq_codel\0", 7);
    invalid_handle[8] = 1;
    assert!(decode_qdiscs(vec![invalid_handle], 7).is_err());
}
#[test]
fn invalid_interface_and_write_commands_rejected_before_ioctl() {
    assert!(interface_index("").is_err());
    assert!(interface_index("1234567890123456").is_err());
    assert!(interface_index("a\0b").is_err());
    let fd = socket(libc::AF_UNIX, 0).unwrap();
    assert!(ethtool_get(&fd, "lo", &mut [0x3b, 0]).is_err()); // SFEATURES
    assert!(ethtool_get(&fd, "lo", &mut []).is_err());
    assert!(route_get(16, 1, false).is_err()); // RTM_NEWLINK
    assert!(route_get(36, 1, true).is_err()); // RTM_NEWQDISC
    assert!(route_get_at(44, 1, true, 0).is_err()); // RTM_NEWTFILTER
    assert!(route_get_at(46, 1, true, 1).is_err()); // unsupported class parent
}
#[test]
fn read_only_linux_get_smoke_without_attach_or_feature_set() {
    // Container-local lo only. This exercises ABI/transport, not XDP support.
    let index = interface_index("lo").unwrap();
    let rows = route_get(18, index, false).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(u32_at(&rows[0], 4).unwrap(), index);
    read_qdiscs(index).unwrap();
    read_features("lo").unwrap().admit().unwrap();
    // loopback is deliberately outside the admitted Ethernet environment.
    assert!(Guard::before("lo").is_err());
    for mode in [AttachMode::Default, AttachMode::Generic] {
        // Must fail before even opening/loading the nonexistent BPF object.
        let result = super::super::kernel::Kernel::attach(
            "/nonexistent/never-open.o",
            "lo",
            "192.0.2.1".parse().unwrap(),
            443,
            mode,
        );
        let error = result.err().unwrap().to_string();
        assert!(error.contains("NIC preservation preflight"), "{error}");
    }
}
