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

Code, history, existing uncommitted work, and local trial evidence are preserved. The GitHub code is not identical to later local changes and trial records. **The instructions, designs, and roadmaps are historical records. See the [validation status and public-code evidence](docs/VALIDATION_STATUS.md) (Japanese) for implemented, simulated, unverified and unmet scopes. They are not deployment recommendations or plans to resume development.**

---

## 🛡️ veil-mesh-consensus

[![CI Checks](https://github.com/mt4110/veil-mesh-consensus/actions/workflows/ci.yml/badge.svg)](https://github.com/mt4110/veil-mesh-consensus/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/mt4110/veil-mesh-consensus)](https://github.com/mt4110/veil-mesh-consensus/releases)

CI badges and release numbers are historical entry points; they do not establish a real-kernel run or production acceptance.

[日本語 (Main)](README.md) · [🧭 Historical Design & Ended Roadmap](docs/V1_ARCHITECTURE_AND_ROADMAP.md) · [📋 Historical Review & Incomplete Proposals](docs/FEASIBILITY_REVIEW_AND_V1_DESIGN.md) · [☁️ Historical Cloud/Edge Proposals (Deployment and Integration Unverified)](docs/CLOUD_DEPLOYMENT_GUIDE.md) · [🧪 Scenario Tests & Limitations](docs/CONSENSUS_AND_EBPF_PROOF.md) · [🔒 Security](SECURITY.md) · [🤝 Contributing](CONTRIBUTING.md)

An experimental prototype containing a signed consensus state machine, mTLS QUIC, IPv4 XDP and a TUI. Component implementations and scenario tests do not establish an operational defense mesh that tolerates compromised servers.

![Historical TUI experiment diagram; not a defense or zero-leakage guarantee](./assets/extermination-flow.svg)

---

## 🎬 Historical Isolated-veth and TUI Demo

<!-- rgc:start -->
![Historical isolated-veth and single-process TUI demo](./assets/demo.gif)
<!-- rgc:end -->

The TUI uses four state machines and demo keys in one process. This is not evidence of four independent hosts, automatic attack detection, production acceptance or general BFT safety.

- `[a]` starts a test-owned process: UDP generation in the dedicated Linux namespace, otherwise `sleep`. A roughly 1 ms wait is a program setting, not a measured 1,000 pkt/s rate.
- `[b]` injects a forged RPC into the state machines. It does not establish real-world attack detection.
- `[x]` drives the in-process consensus demonstration, bridge updates where connected, dummy-buffer clearing and termination of a test-owned process. Actual kernel effects must be checked independently. No application-secret erasure or host-wide protection was demonstrated.

---

## 🛡️ Implementation, Test Scope and Unmet Conditions

See the [public-code validation status](docs/VALIDATION_STATUS.md) and [scenario-test guide](docs/CONSENSUS_AND_EBPF_PROOF.md) (Japanese). `proof_*` names do not constitute mathematical proofs of BFT safety.

| Area | Present in public code | Limit |
| --- | --- | --- |
| Signatures, QC, prefix hashes | Validation functions and selected regression cases | No general BFT, input-truth or false-positive guarantee |
| mTLS, shared PKI, WAL | Read/write paths and scenario tests | Secure key distribution, durability before every response and independent-host operation remain unaccepted |
| View-Change | TimeoutVote/TC state-machine functions and explicitly driven tests | No completed automatic timeout/censorship runtime or complete two-phase-QC BFT |
| TTL, emergency bypass, metrics | User-space functions and tests | No kernel expiry guarantee, kernel bypass operation, recovery CLI or HTTP exporter |
| XDP | Exact source-IPv4 HASH map; historical isolated-veth evidence | No IPv6/CIDR, kernel TTL, all-NIC support or rebuilt-object/source identity guarantee |
| LSM and secret clearing | Rust hook, optional attachment path and dummy-buffer clearing | No demonstrated application-secret erasure, PHP/host protection or zero leakage |
| Guardrails | Exact IPv4 and selected bounds checks | No automatic protection of all management networks, peers or gateways |

Root-compromised hosts, quorum-key compromise and upstream bandwidth saturation remain outside protection. Source spoofing, NAT and shared proxies can cause incorrect blocking. Consensus cannot make a detector's input true.

Warden, Blackhole, veil-rs, Arena, WAF and Nginx integrations were historical ideas; an accepted end-to-end detection→consensus→enforcement→audit integration was not confirmed. Another project's evidence is not evidence for this mesh.

---

## 🧪 Scenario Tests and Historical Check Scripts

`proof_1`–`proof_14` and regression cases cover different scopes: state machines, loopback QUIC in one process, mocks and a conditional Linux-veth test. They do not establish general safety.

```bash
# Historical development checks; inspect skipped and conditional checks separately
bash scripts/check.sh

# Dedicated historical host only; can change remote network and tracing state
bash scripts/deploy-pi-node.sh test

# Saves cargo output and a fixed-format summary
bash scripts/arena-smoke.sh
```

`proof_7` returns `Ok(())` when prerequisites are missing or attachment fails. Green CI or the smoke script's PASS summary does not establish a real-kernel run. `check.sh` omits ShellCheck when it cannot find it. This documentation audit did not rerun product or live-kernel tests. Nix MicroVM configurations and old commands likewise do not establish a working two-VM defense mesh.

---

## ☁️ Cloud & Edge Deployment Matrix (AWS / GCP / Azure / Cloudflare / Vercel)

The AWS/GCP measurements below concern a later local `veil-local-ban` implementation, not the public main defense mesh. Provider XDP capability alone does not establish successful deployment. See the [public AWS/GCP trial summary](docs/review/2026-10-27-CLOUD-VALIDATION-SUMMARY.md) (Japanese). The [historical deployment guide](docs/CLOUD_DEPLOYMENT_GUIDE.md) describes proposed configurations, not validated deployment procedures.

| Platform | Deployment / integration validation | Trial result / historical proposal | Historical guide |
| :--- | :---: | :--- | :--- |
| **AWS** (EC2 / EKS) | **Real-NIC attachment failed in the local EC2 trial** | Isolated XDP passed; ENA rejected attachment at MTU 9001. EKS and other configurations unverified | [AWS Historical Proposal](docs/CLOUD_DEPLOYMENT_GUIDE.md#3-aws-amazon-ec2--vpc-への実践導入手順) |
| **GCP** (Compute Engine / GKE) | **Real-NIC attachment failed in the local Compute Engine trial** | Isolated XDP passed; VirtIO-Net at MTU 1460 returned EINVAL. gVNIC, GKE and other configurations unverified | [GCP Historical Proposal](docs/CLOUD_DEPLOYMENT_GUIDE.md#4-gcp-google-compute-engine--vpc-への実践導入手順) |
| **Microsoft Azure** (Azure VM / AKS) | **Unverified** | Deployment of this implementation, including MANA / Accelerated Networking, has not been validated | [Azure Historical Proposal](docs/CLOUD_DEPLOYMENT_GUIDE.md#5-microsoft-azure-azure-virtual-machines--vnet-への実践導入手順) |
| **Cloudflare** (WAF / Workers) | **Integration unverified** | Historical proposal; API block synchronization and origin-protection integration implementation and acceptance records have not been confirmed | [Cloudflare Historical Proposal](docs/CLOUD_DEPLOYMENT_GUIDE.md#6-cloudflare--vercel-を使う場合のハイブリッド実践構成) |
| **Vercel** (Next.js / Edge) | **Integration unverified** | Historical separate-Linux-backend proposal; integration implementation and acceptance records have not been confirmed | [Vercel Historical Proposal](docs/CLOUD_DEPLOYMENT_GUIDE.md#6-cloudflare--vercel-を使う場合のハイブリッド実践構成) |
