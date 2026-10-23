#!/usr/bin/env bash
# ==============================================================================
# veil-mesh-consensus: pi-node (192.168.1.100) Real Kernel XDP & TUI Arena Runner
# ==============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

MODE="${1:-tui}" # tui | tui-enp | test | sync

echo "=== [1/3] Preparing pi-node (/nix bind-mount & veth-victim testbed) ==="
ssh pi-node '
  if [ ! -d /nix/store ]; then
    sudo mkdir -p /nix
    sudo mount --bind /home/maakie/veil-arena-validation/run-1791624441124999000/results/kvm-runtime-copy/nix /nix
  fi
  if ! ip link show veth-victim >/dev/null 2>&1; then
    sudo ip netns del ns-veil-rogue 2>/dev/null || true
    sudo ip netns add ns-veil-rogue
    sudo ip link add veth-victim type veth peer name veth-rogue
    sudo ip link set veth-rogue netns ns-veil-rogue
    sudo ip addr add 10.99.0.1/24 dev veth-victim
    sudo ip link set veth-victim up
    sudo ip netns exec ns-veil-rogue ip addr add 10.99.0.66/24 dev veth-rogue
    sudo ip netns exec ns-veil-rogue ip link set veth-rogue up
  fi
'

if [ "$MODE" = "test" ]; then
  echo "=== [2/3] Running All 7 Proof Tests (Including Live Kernel XDP_DROP on pi-node) ==="
  TEST_BIN="$(ssh pi-node "ls -t ~/veil-mesh-consensus/target/debug/deps/veil_node-* | head -n 1")"
  ssh pi-node "sudo $TEST_BIN --nocapture --test-threads=1"
  exit 0
fi

IFACE="veth-victim"
if [ "$MODE" = "tui-enp" ]; then
  IFACE="enp0s31f6"
fi

echo "=== [2/3] Attaching Live eBPF XDP (`veil_xdp_kern.o`) to `$IFACE` & Launching TUI Arena ==="
echo "  Controls inside TUI: [a] Spawn UDP Flood Process -> [b] Byzantine Spoof -> [x] Consensus Exterminate -> [q] Quit"
ssh -t pi-node "sudo ~/veil-mesh-consensus/target/release/veil-node --tui --iface $IFACE --ebpf-obj ~/veil-mesh-consensus/assets/veil_xdp_kern.o"
