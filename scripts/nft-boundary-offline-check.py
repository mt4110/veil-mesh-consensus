#!/usr/bin/env python3
"""Run only the dependency-free offline experiment. No nft/sudo/SSH/cloud calls."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "experiments/nft-boundary/Cargo.toml"
PRESERVED = ["Cargo.toml", "Cargo.lock", "veil-node/Cargo.toml",
             "veil-node/src/local_ban/mod.rs", "veil-node/src/local_ban/kernel.rs",
             "veil-node/src/local_ban/runtime.rs", "assets/veil_xdp_kern.o"]


def hashes():
    return {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in PRESERVED}


def main():
    os.umask(0o077)
    evidence = Path(tempfile.mkdtemp(prefix="nft-boundary-check-", dir=ROOT / "artifacts"))
    target = evidence / "target"
    before = hashes()
    result = {"started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "mode": "offline-model-only", "kernel_contacted": False,
              "apply_supported": False, "cases": [], "production_before": before}
    env = os.environ.copy()
    env["CARGO_NET_OFFLINE"] = "true"
    common = ["--offline", "--locked", "--manifest-path", str(MANIFEST), "--target-dir", str(target)]

    def execute(name, argv, expected=0):
        with (evidence / (name + ".stdout")).open("xb") as out, (evidence / (name + ".stderr")).open("xb") as err:
            completed = subprocess.run(argv, cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
                                       stdout=out, stderr=err, timeout=180)
        result["cases"].append({"name": name, "argv": argv, "exit": completed.returncode,
                                "expected_exit": expected})
        if completed.returncode != expected:
            raise RuntimeError(f"{name}: exit={completed.returncode}, expected={expected}")
        print(f"PASS {name}", flush=True)

    try:
        execute("format", ["cargo", "fmt", "--manifest-path", str(MANIFEST), "--", "--check"])
        execute("tests", ["cargo", "test", *common])
        execute("clippy", ["cargo", "clippy", *common, "--all-targets", "--", "-D", "warnings"])
        execute("demo", ["cargo", "run", *common, "--", "offline-demo"])
        demo = (evidence / "demo.stdout").read_text()
        required = ["mode=offline-model-only", "kernel_contacted=false", "apply_supported=false",
                    "after_ack_loss=Quarantined", "cleanup_before_absence_verified=Closing",
                    "model_cleanup_after_absence_verified=Stopped", "deployment_gate=BLOCKED",
                    "absolute-CLOCK_BOOTTIME-deadline", "intended_boot_deadline_ns=30000000000",
                    "naive_relative_deadline_ns=31000000000"]
        if not all(part in demo for part in required):
            raise RuntimeError("Demo did not retain its evidence/gate contract")
        binary = target / "debug/nft-boundary-offline"
        execute("apply_refused", [str(binary), "--apply"], expected=2)
        execute("no_implicit_demo", [str(binary)], expected=2)
        result["offline_checks_complete"] = True
    except Exception as error:
        result["error"] = repr(error)
        raise
    finally:
        result["production_after"] = hashes()
        result["production_preserved"] = before == result["production_after"]
        result["complete"] = result.get("offline_checks_complete", False) and result["production_preserved"]
        result["ended_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        (evidence / "result.json").write_text(json.dumps(result, indent=2))
        print("EVIDENCE=" + str(evidence), flush=True)
        if not result["production_preserved"]:
            raise RuntimeError("Existing product hashes changed; preserve and inspect")


if __name__ == "__main__":
    main()
