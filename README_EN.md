# 🛡️ veil-mesh-consensus

[![CI Checks](https://github.com/mt4110/veil-mesh-consensus/actions/workflows/ci.yml/badge.svg)](https://github.com/mt4110/veil-mesh-consensus/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/mt4110/veil-mesh-consensus)](https://github.com/mt4110/veil-mesh-consensus/releases)

[日本語 (Main)](README.md) · [🧭 v0.9.1 Architecture & Roadmap](docs/V1_ARCHITECTURE_AND_ROADMAP.md) · [📋 Feasibility Review & Design](docs/FEASIBILITY_REVIEW_AND_V1_DESIGN.md) · [☁️ Cloud Deployment Guide (AWS/GCP/Azure/CF/Vercel)](docs/CLOUD_DEPLOYMENT_GUIDE.md) · [🧪 Proof Documentation](docs/CONSENSUS_AND_EBPF_PROOF.md) · [🔒 Security](SECURITY.md) · [🤝 Contributing](CONTRIBUTING.md)

**An experimental prototype that synchronizes block rules decided by signed Raft-based consensus (Ed25519 `SignedConsensusRpc` + Multi-Signature `QuorumCertificate` over mTLS QUIC) into Linux kernel eBPF (`Aya` XDP packet filtering) and a user-space LSM simulation bridge.**

![Autonomous Attack Extermination Flow](./assets/extermination-flow.svg)

---

## 🎬 Live Hardware Demo (`iftop` Bandwidth Monitor & Real Kernel XDP + TUI Arena)

<!-- rgc:start -->
![Demo](./assets/demo.gif)
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

## 🛡️ Security Model: Verified Protections & Known Limitations

This project is an **experimental prototype exploring consensus-driven eBPF policy enforcement**. Below is an honest breakdown of what is cryptographically enforced and tested in code (`proof_1`–`proof_11`), alongside current limitations.

### 1. Verified Protections in Code (`proof_1`–`proof_14` + `regression_r1`–`r10`)

| Threat Scenario | Enforcement Status | Implementation & Regression Proof |
| :--- | :---: | :--- |
| **Unsigned, spoofed, or tampered RPCs on the QUIC wire** | **Enforced & Tested** | `WireMessage::Consensus` exclusively carries `SignedConsensusRpc`, and [`handle_connection`](veil-node/src/infra/quic_mtls.rs) always validates messages via [`handle_signed_rpc`](veil-node/src/usecase/consensus_engine.rs) (`proof_5`, `proof_9`). |
| **Untrusted TLS peers & Multi-Process Shared Root CA** | **Enforced & Tested** | [`ClusterMtlsPki`](veil-node/src/infra/quic_mtls.rs) enforces mutual TLS 1.3 and supports shared multi-process PKI, peer public key auto-discovery, and Crash-Recovery WAL (`proof_9`, `regression_r3`, `regression_wal`). |
| **Prefix tampering via tail QC (R1) & Cross-term conflicting commits (R2)** | **Enforced & Tested** | Cumulative hash chaining ([`digest_with_parent`](veil-node/src/domain/entities.rs)), Voted-Entry Locks, and double-vote prevention (`voted_entry_hashes`) prevent conflicting commits (`proof_8`, `regression_r1`, `regression_r2`). |
| **Silent/censoring leader stalls (View-Change)** | **Enforced & Tested** | $\lfloor 2N/3 \rfloor + 1$ `TimeoutVote`s form a [`TimeoutCertificate`](veil-node/src/domain/entities.rs) (TC) to deterministically rotate to the next round-robin leader (`proof_12`, `proof_14`). |
| **Relative TTL auto-expiry, micro-batching & emergency local bypass** | **Enforced & Tested** | Monotonic-clock TTL auto-expiry, `SetPeerTrustBatch` (up to 128 rules), emergency local bypass, and Prometheus metrics are implemented and verified (`proof_13`). |
| **Boundary attacks (`index = 0` / `term >= MAX_SAFE_TERM`), `0600` key perms & chunking** | **Enforced & Tested** | Pre-validation rejects out-of-bounds indices/terms, enforces `0600` key permissions, and chunks catch-up batches (`proof_11`, `regression_r8`, `regression_r9`, `regression_r10`). |

### 2. Known Limitations & Out-of-Scope Risks

- **Real Kernel XDP vs. Simulated User-Space LSM/Stealth GC**: The IPv4 `XDP_DROP` packet filter (`veil_xdp_kern.c` / `PEER_POLICY_MAP`) is tested on a live Linux kernel (`proof_7`), whereas the LSM `socket_connect` lockdown and Stealth GC key zeroization are currently simulated in user space via `MockEbpfBridge`.
- **IPv4-Only XDP & Source IP Spoofing**: The current XDP program inspects IPv4 source addresses (`src_ip`) only (IPv6 passes through). Because L3 source IPs can be spoofed in raw UDP packets, production deployments should feed consensus proposals from authenticated L7 WAF or TCP-established host sensors ([`veil-warden`](https://github.com/mt4110/veil-warden)) rather than unverified UDP source IPs.
- **Shared Credentials & Host Root Compromise**: Consensus cannot prevent data exfiltration inside a node where `root` is already compromised, nor can it survive simultaneous compromise of $\ge 2f+1$ nodes sharing identical admin credentials.

---

## 🧪 11 Verification & Regression Proof Tests

```bash
# Run Markdown lint, ShellCheck, Clippy, and all proof tests
bash scripts/check.sh

# Run all proof tests including live Linux kernel XDP_DROP on pi-node
bash scripts/deploy-pi-node.sh test

# Save verification evidence in veil-arena format
bash scripts/arena-smoke.sh
```

---

## ☁️ Cloud & Edge Deployment Matrix (AWS / GCP / Azure / Cloudflare / Vercel)

See **[docs/CLOUD_DEPLOYMENT_GUIDE.md](docs/CLOUD_DEPLOYMENT_GUIDE.md)** for full step-by-step instructions.

| Platform | Direct Kernel Support | Recommended Role | Guide Link |
| :--- | :---: | :--- | :--- |
| **AWS** (EC2 / EKS) | **Yes** | Nitro ENA Native XDP Defense Mesh | [AWS Guide](docs/CLOUD_DEPLOYMENT_GUIDE.md#3-aws-amazon-ec2--vpc-への実践導入手順) |
| **GCP** (Compute Engine / GKE) | **Yes** | gVNIC XDP Defense Mesh | [GCP Guide](docs/CLOUD_DEPLOYMENT_GUIDE.md#4-gcp-google-compute-engine--vpc-への実践導入手順) |
| **Microsoft Azure** (Azure VM / AKS) | **Yes** | MANA / Accelerated Networking XDP | [Azure Guide](docs/CLOUD_DEPLOYMENT_GUIDE.md#5-microsoft-azure-azure-virtual-machines--vnet-への実践導入手順) |
| **Cloudflare** (WAF / Workers) | **Hybrid** | Edge L7 Sensor + Origin XDP Protection & API Sync | [Cloudflare Guide](docs/CLOUD_DEPLOYMENT_GUIDE.md#6-cloudflare--vercel-を使う場合のハイブリッド実践構成) |
| **Vercel** (Next.js / Edge) | **Hybrid Only** | Frontend Hosting + Backend Linux Mesh Protection | [Vercel Guide](docs/CLOUD_DEPLOYMENT_GUIDE.md#6-cloudflare--vercel-を使う場合のハイブリッド実践構成) |
