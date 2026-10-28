Archived local working document / ローカル最終文書の保全。

2026-10-28: Preserved from the local changes involved in the stash conflict. Historical statements, next steps, and relative links below retain their original context; they are not current recommendations or authorization to resume development. See the repository root README and docs/PROJECT_CLOSURE.md for closure status.

---

# DEVELOPMENT DISCONTINUED — veil-mesh-consensus

> [!CAUTION]
> **Development ended on October 27, 2026. This repository preserves experimental research. Production deployment is not recommended.**
>
> **No further maintenance, features, or security fixes are planned. The GitHub repository is retained as a read-only archive.**

## Why development ended

- The original goal was cooperative defense across servers without allowing one compromised server to cause unjustified blocking on healthy peers. Practical fulfillment of that goal was not demonstrated. The later single-host manual temporary-BAN tool does not fulfill it.
- Privileged operations and dependencies on kernels, NICs, drivers, and cloud runtimes impose deployment and maintenance costs. Benefits sufficient to justify those costs over existing alternatives were not demonstrated.
- Isolated tests and selected-host successes and failures remain research evidence. General cloud compatibility and safe public-site pilot acceptance were not achieved. Further development and paid experiments have ended.

This is not a claim that all software requiring sudo is inappropriate or that GCP and AWS prohibit eBPF in general. The decision concerns this project's goal, implementation, deployment burden, and demonstrated benefits.

Code, history, existing uncommitted work, and local trial evidence are preserved. The GitHub code is not identical to later local changes and trial records. **The installation instructions, designs, and roadmaps below are historical experiment records, not current deployment recommendations or plans to resume development.**

---

## veil-mesh-consensus — local temporary BANs

The current pilot candidate is **`veil-local-ban`**, an optional Linux component for manually blocking a source IPv4 address on one destination IPv4/TCP port for a finite time.

**At most 256 concurrent BANs. New entries are rejected at capacity. Every BAN disappears on restart.** This mode has no automatic detection, automatic BANs, distributed consensus or persistent BAN recovery. Ordinary browsers need no custom client or token.

[日本語](../../../README.md) · [Installation](../../local-ban/INSTALL.md) · [Operations](../../local-ban/OPERATIONS.md) · [Recovery](../../local-ban/RECOVERY.md) · [Pilot checklist](../../local-ban/PILOT_CHECKLIST.md) · [Test history](../../local-ban/TEST_HISTORY.md) · [Security and reporting](../../../SECURITY.md)

The detailed operator guides linked above are currently in Japanese.

## Evidence and deployment status

2026-10-27: [Explicit Generic mode and an active/wanted offload guard](../../review/2026-10-27-GENERIC-OFFLOAD-GUARD-IMPLEMENTATION.md) are implemented and tested offline. Both startup modes require LRO and hardware GRO already off in active and wanted state, with ownership/mode/feature checks before READY. The non-root Linux local-BAN suite passed 51 tests. There is no automatic fallback or NIC configuration. **The new loader has not passed real-XDP, real-NIC or GCP acceptance; the public pilot remains on hold.** Previous binary and trial evidence remain preserved. New NIC configuration and paid trials require a separate concrete plan and approval.

As of 2026-10-26, these are working-tree validation records, not evidence that a public Release or current main CI passed. The Japanese [test history](../../local-ban/TEST_HISTORY.md) links the preserved BFT investigations, isolated XDP and physical-LAN results, aborted WAN trials, and GCP preparation. See the [acceptance criteria and stopping limits](../../local-ban/DELIVERY_GATE.md) for the pilot decision.

The [two-VM GCP proposal](../../local-ban/GCP_TWO_VM_EXECUTION_PLAN.md) records the wired-Mac control path, limited ingress, candidate domain, costs, evidence export and removal tradeoffs. After scope A failed, [one authorized comparison in a second zone also returned HTTP 503; the new network resources were removed and baseline checks passed](../../review/2026-10-26-GCP-A-ZONE-B-COMPARISON.md). No VMs were created or XDP tests run. Public scope B remains on hold.

A subsequent [read-only request and audit-log analysis](../../review/2026-10-26-GCP-503-REQUEST-ANALYSIS.md) found no obvious error in the inspected request fields. The root cause remains unconfirmed, no additional creation was attempted, and the report records the four remaining stages and completion criteria.

| Scope | Status |
| --- | --- |
| List, preview, explicit apply/unban, stale instance/revision refusal | Verified using the new binary on isolated pi-node Linux, veth, real XDP and HTTPS |
| 256 entries, entry 257 rejection, kernel expiry, normal/SIGKILL restart | Verified in the same real-XDP run |
| Permanent units, dedicated account, damaged ownership recovery and host boot | Earlier isolated records with earlier binaries/fixtures; not all rerun with the latest binary |
| Physical eno1/e1000e on hp-ubuntu | The same binary passed one LAN BAN, explicit unban before expiry, normal-client/new-SSH/control-port continuity and stop recovery using Generic XDP |
| Home WAN / GCP | WAN BAN/unban were not performed; additional home trials are on hold. GCP VM creation returned HTTP 503 in two zones, new network resources were removed, and no GCP XDP or traffic trial was performed |
| Public site, other NICs/native mode, CDN paths, operator usability and adoption value | Unverified; public deployment remains on hold |

The [isolated kernel acceptance](../../review/2026-10-25-LOCAL-BAN-OPERATOR-KERNEL-ACCEPTANCE.md) is now complemented by [one physical-eno1 LAN trial](../../review/2026-10-25-HP-UBUNTU-PHYSICAL-NIC-ACCEPTANCE.md), with 23 functional checks and an independent final audit. An observer-process false positive in the trial driver was diagnosed while preserving its original result. Artifact hashes, stopping, recovery and preservation were checked; this does not establish support for all NICs, native performance or public deployment.

The earlier [host-boot record](../../review/2026-10-25-LOCAL-BAN-BOOT-ACCEPTANCE.md) confirmed namespace restoration and empty BAN state, but retained a failed same-container-ID audit and a Cargo launch failure. The later [build recovery](../../review/2026-10-25-NIX-BUILD-RECOVERY.md) is separate; neither establishes that the whole host was unaffected.

## Contract and limits

- One exact destination IPv4/TCP port with the real client source visible at ingress. IP blocking also affects other users sharing that source address; it cannot distinguish HTTP users or paths.
- TTL is 1–86,400 seconds. Kernel boot-clock expiry does not depend on userspace cleanup. Restart starts a new instance with no BANs.
- Cleanup examines at most 16 slots per 200 ms. Unknown packet sources do not allocate BAN entries. These limits are not hard latency or bandwidth guarantees.
- Administration uses a dedicated UID and local Unix sockets. List/preview are read-only; changes require an explicit command with fixed instance/revision. Unknown outcomes are never automatically retried.
- IPv6, UDP/HTTP3, VLAN and fragmented IPv4 pass outside this additional profile. Keep the baseline firewall and HTTPS/HTTP controls.
- Do not BAN a CDN or remote L7 proxy in place of its users. This component does not solve upstream bandwidth exhaustion or application vulnerabilities.
- Manual operation can help with a few already-identified sources. It is insufficient for unattended protection; benefit over existing defenses remains unproven.
- Root compromise is outside coverage. A compromised service UID can also misuse this host's BAN authority.

## Pilot path

Complete the [checklist](../../local-ban/PILOT_CHECKLIST.md), review [installation and rollback](../../local-ban/INSTALL.md), then obtain approval for the concrete target changes. Validate the selected binary on the isolated host, the LAN/physical ingress, and one approved public canary in that order.
Use [list → preview → explicit apply/unban](../../local-ban/OPERATIONS.md), independently verify communication, then follow [recovery and stopping](../../local-ban/RECOVERY.md). End the first trial stopped and not enabled; continued adoption is a separate value decision.
These documents are not an unattended installer. Do not copy example IPs, pi-node fixtures or legacy demos into a public service. A dual-boot machine cannot serve the Ubuntu site while Windows is running.

## Development and preserved BFT records

Select the Linux binary explicitly; see [installation](../../local-ban/INSTALL.md) for the build environment and tested C XDP object.

```sh
cargo build --locked --release -p veil-node --bin veil-local-ban
cargo test --locked -p veil-node --lib
```

Use a new build workspace to preserve existing outputs. macOS library tests cover portable logic, not Linux/XDP. The default `cargo run -p veil-node` still selects the legacy BFT binary.
The previous README, BFT/QUIC/WAL and demo instructions remain preserved below as historical experiment records. They are not the local-BAN operator contract. No old WAL or signing key is migrated or deleted.

<details>
<summary>Open the preserved README and BFT experiment records</summary>

## 🛡️ veil-mesh-consensus

> Local temporary BAN mode (2026-10-24): `veil-local-ban` adds optional, host-local enforcement for one IPv4/TCP service, with 256 slots, bounded cleanup and capacity rejection. **Restart clears every BAN.** Existing BFT/WAL operation is preserved. See the [contract, commands, isolated Linux evidence and limitations](../../review/2026-10-24-LOCAL-EPHEMERAL-BAN-IMPLEMENTATION.md). [Recovery after SIGKILL](../../review/2026-10-24-LOCAL-BAN-CRASH-RESTART.md) now reclaims only verified owned sockets; unknown paths are preserved. See [read-only inspection and manual recovery](../../review/2026-10-24-LOCAL-BAN-MANUAL-RECOVERY.md) for interrupted initialization or damaged ownership records. See [systemd operation and startup privilege dropping](../../review/2026-10-24-LOCAL-BAN-SYSTEMD-OPERATIONS.md) for lifecycle, bounded restart and recovery using the same unit.

<!-- preserved-quote-boundary -->

> Authorization update (2026-10-24): Normal daemons require an independent operator signature and exact destination IPv4/TCP-or-UDP/port scopes. Individual unban preserves a checkpointed revision fence. Context 6 / WAL v9; legacy WALs are preserved and refused. IPv6, VLAN and fragmented IPv4 are outside coverage and pass. The [current contract and commands](../../review/2026-10-24-SCOPED-OPERATOR-APPROVAL.md) supersede earlier raw-BAN instructions.
>
> WAL update (2026-10-24): Certified checkpoints, state transfer and agreed expiry retirement now bound retained history. Normal daemons use context 6 / WAL v9 and refuse automatic legacy WAL conversion. The [contract, limits and evidence](../../review/2026-10-24-BOUNDED-WAL-CHECKPOINTS.md) supersedes earlier storage descriptions.
>
> Two-phase update (2026-10-24): Normal daemons use separate prepare/commit certificates and signed new-view reports. Legacy WALs are preserved and refused, never automatically reinterpreted. Consensus context 6 / WAL v9; one outstanding height, up to 7 members, 1024-byte commands. See the [safety, compatibility and evidence record](../../review/2026-10-24-TWO-PHASE-VIEW-CHANGE.md).

[![CI Checks](https://github.com/mt4110/veil-mesh-consensus/actions/workflows/ci.yml/badge.svg)](https://github.com/mt4110/veil-mesh-consensus/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](../../../LICENSE)
[![Release](https://img.shields.io/github/v/release/mt4110/veil-mesh-consensus)](https://github.com/mt4110/veil-mesh-consensus/releases)

[日本語 (Main)](../../../README.md) · [🧭 v0.9.1 Architecture & Roadmap](../../V1_ARCHITECTURE_AND_ROADMAP.md) · [📋 Feasibility Review & Design](../../FEASIBILITY_REVIEW_AND_V1_DESIGN.md) · [☁️ Historical Cloud/Edge Proposals (Deployment and Integration Unverified)](../../CLOUD_DEPLOYMENT_GUIDE.md) · [🧪 Proof Documentation](../../CONSENSUS_AND_EBPF_PROOF.md) · [🔒 Security](../../../SECURITY.md) · [🤝 Contributing](../../../CONTRIBUTING.md)

**An experimental prototype that synchronizes block rules decided by signed Raft-based consensus (Ed25519 `SignedConsensusRpc` + Multi-Signature `QuorumCertificate` over mTLS QUIC) into Linux kernel eBPF (`Aya` XDP packet filtering) and a user-space LSM simulation bridge.**

> **Deployment status (2026-10-24):** Production automatic blocking remains on hold. The current fixes address rejected-QC log mutation, responses before persistence, apply retries, and wire-size limits. Kernel TTL/bypass and expiry-preserving WAL recovery have also passed isolated Linux tests. Signed common deadlines also passed delayed catch-up tests. RPC/QC/timeout signatures now bind cluster ID, config epoch and a canonical configuration digest. Fixed voter membership and configuration mismatch rejection passed real mTLS QUIC tests. Automatic elections, local BAN acceptance, commit/apply, and expiry now pass a four-process isolated Linux daemon trial. Normal daemons now load node-specific TLS certificates and keys without a CA private key. Offline issuance, rotation and local revocation-list rollout passed isolated tests. BFT liveness, multi-host rollout and site acceptance remain incomplete. See the [initial fixes](../../review/2026-10-23-CRITICAL-FIXES.md), [TTL/bypass recovery evidence](../../review/2026-10-23-TTL-BYPASS-RECOVERY.md), and [signed deadline tests](../../review/2026-10-23-SIGNED-DEADLINE-CATCHUP.md), and [cluster context binding](../../review/2026-10-23-CLUSTER-CONTEXT-BINDING.md), and [fixed membership, configuration digest and startup compatibility](../../review/2026-10-24-FIXED-MEMBERSHIP-DIGEST.md).

See the [normal-daemon operation contract and evidence](../../review/2026-10-24-DAEMON-OPERATION-FLOW.md). A successful `--submit-approval FILE` command returns a durable **acceptance** receipt, not confirmation of commit or enforcement. Check `--status-local --entry-index ... --status-ip ...`, compare the receipt hash, and inspect the backend and errors. Non-Linux daemons refuse new TTL proposals rather than using simulator time.

Normal daemon startup also requires `--tls-dir`: a pre-provisioned node bundle. There is no shared-CA or key-generation fallback. See [TLS issuance, preflight, rotation, revocation and migration](../../review/2026-10-24-NODE-TLS-OPERATIONS.md). Revocation requires distributing an updated bundle and restarting each verifier.

IP BAN entries now require a finite TTL and signed common expiry throughout consensus. Legacy and zero-TTL BANs are rejected; WALs containing them are preserved and refused at startup. Unban commands remain usable without expiry. See [finite BAN policy and recovery boundaries](../../review/2026-10-24-FINITE-BAN-POLICY.md).

Normal daemon startup requires the complete `--member ID@IP:PORT=HEX_PUBLIC_KEY` voter list and an existing signing key. `--peer` and automatic peer-key discovery are no longer supported. Provision new trial keys explicitly with `--init-key`, then create their initial WAL once with `--init-state`. Normal startup requires an existing valid WAL; it never recreates lost voting history. See [bootstrap and recovery boundaries](../../review/2026-10-24-WAL-BOOTSTRAP-RECOVERY.md). Do not replace an existing validator identity to bypass recovery.

![Autonomous Attack Extermination Flow](../../../assets/extermination-flow.svg)

---

### 🎬 Live Hardware Demo (`iftop` Bandwidth Monitor & Real Kernel XDP + TUI Arena)

<!-- rgc:start -->
![Demo](../../../assets/demo.gif)
<!-- rgc:end -->

> **🎥 What Happens in the Demo GIF (`[a]` $\to$ `[b]` $\to$ `[x]` Key Sequence)**
>
> - **Left Terminal (`sudo iftop -i veth-victim`)**: Real-time network bandwidth monitor on a physical Linux host (`pi-node`: Kernel 7.0 x86_64).
> - **Right Terminal (`veil-node --tui`)**: TUI Arena with live eBPF bytecode (`veil_xdp_kern.o`) attached to the kernel `veth-victim` interface.
>
> 1. **Press `[a]` (Spawn Rogue Attack Process)**: Spawns an OS process inside `ns-veil-rogue` (`10.99.0.66`) sending a 1,000 pkt/s UDP flood at `10.99.0.1:9999`.
> 2. **Press `[b]` (Inject Byzantine Forged RPC)**: Broadcasts a forged `AppendEntries` RPC spoofing `node-alpha` (`Term 999`). Nodes verify and reject the packet via **Ed25519 public key verification**.
> 3. **Press `[x]` (Consensus Commit & Kernel eBPF Sync)**: Once the 4-node cluster reaches a multi-signature Quorum ($\ge 3/4$ Ed25519 QC signatures), `sys_bpf(BPF_MAP_UPDATE_ELEM)` writes `trusted=0` into the kernel `PEER_POLICY_MAP`, triggering **NIC-level `XDP_DROP`**, **user-space LSM / Stealth GC simulation**, and **`sys_kill(PID, SIGKILL=9)`** to stop the mock attack process.

---

### 🛡️ Security Model: Verified Protections & Known Limitations

This project is an **experimental prototype exploring consensus-driven eBPF policy enforcement**. Below is an honest breakdown of what is cryptographically enforced and tested in code (`proof_1`–`proof_11`), alongside current limitations.

#### 1. Verified Protections in Code (`proof_1`–`proof_14` + `regression_r1`–`r10`)

| Threat Scenario | Enforcement Status | Implementation & Regression Proof |
| :--- | :---: | :--- |
| **Unsigned, spoofed, or tampered RPCs on the QUIC wire** | **Enforced & Tested** | `WireMessage::Consensus` exclusively carries `SignedConsensusRpc`, and [`handle_connection`](../../../veil-node/src/infra/quic_mtls.rs) always validates messages via [`handle_signed_rpc`](../../../veil-node/src/usecase/consensus_engine.rs) (`proof_5`, `proof_9`). |
| **TLS, PKI and WAL** | **Isolated tests with operating constraints** | Node-specific TLS, persistence before votes, required existing WAL at normal startup and one-use bootstrap are tested. Whole-disk rollback, signing-key cloning across hosts and production distribution remain unresolved. |
| **Prefix tampering and conflicting commits** | **Known counterexamples covered** | Hash chaining and vote history are supplemented by QC verification against a candidate log before mutation. The rejected-QC counterexample is fixed; this is not a complete BFT proof. |
| **Leader rotation / view change** | **Two-phase / real QUIC regression** | Prepare/commit and signed new-view reports recover conflicting prefixes. Automatic daemon recovery and hidden-commit regressions are covered; general formal proof and long-running deployment validation remain incomplete. |
| **TTL, batching and local bypass** | **Isolated Linux tests passed** | XDP uses boot-clock deadlines; an owner-only socket controls bypass. WAL v9 preserves deadlines, bypass intent, configuration-bound signatures and two-phase voting evidence. The normal daemon rejects legacy WAL without modifying it. Tests cover expiry during `SIGSTOP` and process restart. Signed common deadlines limit catch-up to the remaining lifetime under clock error/rate assumptions. Metrics HTTP remains unimplemented. [Contract and limitations](../../review/2026-10-23-SIGNED-DEADLINE-CATCHUP.md) |
| **Boundary attacks (`index = 0` / `term >= MAX_SAFE_TERM`), `0600` key perms & chunking** | **Enforced & Tested** | Pre-validation rejects out-of-bounds indices/terms, enforces `0600` key permissions, and chunks catch-up batches (`proof_11`, `regression_r8`, `regression_r9`, `regression_r10`). |

#### 2. Known Limitations & Out-of-Scope Risks

- **Real Kernel XDP vs. Simulated User-Space LSM/Stealth GC**: The IPv4 `XDP_DROP` packet filter (`veil_xdp_kern.c` / `PEER_POLICY_MAP`) is tested on a live Linux kernel (`proof_7`), whereas the LSM `socket_connect` lockdown and Stealth GC key zeroization are currently simulated in user space via `MockEbpfBridge`.
- **IPv4-Only XDP & Source IP Spoofing**: The current XDP program inspects IPv4 source addresses (`src_ip`) only (IPv6 passes through). Because L3 source IPs can be spoofed in raw UDP packets, production deployments should feed consensus proposals from authenticated L7 WAF or TCP-established host sensors ([`veil-warden`](https://github.com/mt4110/veil-warden)) rather than unverified UDP source IPs.
- **Shared Credentials & Host Root Compromise**: Consensus cannot prevent data exfiltration inside a node where `root` is already compromised, nor can it survive simultaneous compromise of $\ge 2f+1$ nodes sharing identical admin credentials.

---

### 🧪 11 Verification & Regression Proof Tests

```bash
# Run Markdown lint, ShellCheck, Clippy, and all proof tests
bash scripts/check.sh

# Run all proof tests including live Linux kernel XDP_DROP on pi-node
bash scripts/deploy-pi-node.sh test

# Save verification evidence in veil-arena format
bash scripts/arena-smoke.sh
```

---

### ☁️ Cloud & Edge Deployment Matrix (AWS / GCP / Azure / Cloudflare / Vercel)

The table describes validation of this implementation. Provider XDP capability alone does not establish successful deployment. See the [public AWS/GCP trial summary](../../review/2026-10-27-CLOUD-VALIDATION-SUMMARY.md) (Japanese). The [historical deployment guide](../../CLOUD_DEPLOYMENT_GUIDE.md) describes proposed configurations, not validated deployment procedures.

| Platform | Deployment / integration validation | Trial result / historical proposal | Historical guide |
| :--- | :---: | :--- | :--- |
| **AWS** (EC2 / EKS) | **Real-NIC attachment failed in the local EC2 trial** | Isolated XDP passed; ENA rejected attachment at MTU 9001. EKS and other configurations unverified | [AWS Historical Proposal](../../CLOUD_DEPLOYMENT_GUIDE.md#3-aws-amazon-ec2--vpc-への実践導入手順) |
| **GCP** (Compute Engine / GKE) | **Real-NIC attachment failed in the local Compute Engine trial** | Isolated XDP passed; VirtIO-Net at MTU 1460 returned EINVAL. gVNIC, GKE and other configurations unverified | [GCP Historical Proposal](../../CLOUD_DEPLOYMENT_GUIDE.md#4-gcp-google-compute-engine--vpc-への実践導入手順) |
| **Microsoft Azure** (Azure VM / AKS) | **Unverified** | Deployment of this implementation, including MANA / Accelerated Networking, has not been validated | [Azure Historical Proposal](../../CLOUD_DEPLOYMENT_GUIDE.md#5-microsoft-azure-azure-virtual-machines--vnet-への実践導入手順) |
| **Cloudflare** (WAF / Workers) | **Integration unverified** | Historical proposal; API block synchronization and origin-protection integration implementation and acceptance records have not been confirmed | [Cloudflare Historical Proposal](../../CLOUD_DEPLOYMENT_GUIDE.md#6-cloudflare--vercel-を使う場合のハイブリッド実践構成) |
| **Vercel** (Next.js / Edge) | **Integration unverified** | Historical separate-Linux-backend proposal; integration implementation and acceptance records have not been confirmed | [Vercel Historical Proposal](../../CLOUD_DEPLOYMENT_GUIDE.md#6-cloudflare--vercel-を使う場合のハイブリッド実践構成) |

For witnessed BAN progress, submit `--submit-approval FILE` through any member’s owner-only socket and inspect `--status-local --request-id ID`. A 3-of-4 request timeout can replace a censoring leader without renewing the signed deadline. Receipt is not commit. The normal daemon requires WAL v9 and preserves legacy WAL without automatic migration. See the [contract and validation scope](../../review/2026-10-24-WITNESSED-BAN-PROGRESS.md); this is not a complete Byzantine liveness guarantee.

</details>
