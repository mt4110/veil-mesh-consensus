#![forbid(unsafe_code)]

use nft_boundary_offline::*;
use std::net::Ipv4Addr;

fn main() {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["offline-demo"] {
        eprintln!("Offline experiment only. Usage: nft-boundary-offline offline-demo");
        std::process::exit(2);
    }
    let seen = ObservedContext {
        uid: 1001,
        namespace: 12,
        owner_socket: 42,
    };
    let scope = Scope::new(
        Ipv4Addr::new(10, 99, 0, 66),
        18443,
        seen,
        &[Ipv4Addr::new(198, 51, 100, 7)],
    )
    .unwrap();
    let mut broker = ModelBroker::start(scope, 1, true).unwrap();
    let preview = broker
        .preview(seen, Operation::Ban, Ipv4Addr::new(10, 99, 0, 1), 30, 0)
        .unwrap();
    let transaction = broker.prepare(seen, &preview, 1_000_000_000).unwrap();
    println!("mode=offline-model-only\nkernel_contacted=false\napply_supported=false");
    println!("reviewed_transaction={transaction:?}");
    // Unknown delivery/ACK result must never become APPLIED or an automatic retry.
    assert_eq!(broker.finish(Completion::Unknown), Err(Error::Unknown));
    println!(
        "after_ack_loss={:?}\noccupied={}",
        broker.phase(),
        broker.occupied()
    );
    broker.close_owner(seen).unwrap();
    assert_eq!(
        broker.observe_absent_after_close(seen, false),
        Err(Error::TablePresent)
    );
    println!("cleanup_before_absence_verified={:?}", broker.phase());
    broker.observe_absent_after_close(seen, true).unwrap();
    println!("model_cleanup_after_absence_verified={:?}", broker.phase());
    let evidence = DeploymentEvidence {
        isolated_namespace: false,
        os_peer_credentials_verified: false,
        capabilities_confined: false,
        owner_without_persist_verified: false,
        absolute_boottime_deadline_verified: false,
        recovery_and_packet_tests_verified: false,
    };
    println!("deployment_gate=BLOCKED\nmissing={:?}", evidence.missing());
    let desired = 30_000_000_000;
    let naive = relative_deadline_at_kernel_init(1_000_000_000, desired).unwrap();
    println!("counterexample=request_at_0_ttl_30_kernel_init_at_1\nintended_boot_deadline_ns={desired}\nnaive_relative_deadline_ns={naive}");
}
