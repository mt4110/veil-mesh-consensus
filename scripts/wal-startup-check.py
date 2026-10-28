#!/usr/bin/env python3
"""Black-box initial state/recovery checks; retains all evidence and fixture keys.

Uses local loopback only. Does not attach XDP or delete files.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    root = args.output_dir.resolve()
    root.mkdir(mode=0o700)
    binary = args.binary.resolve()
    checks = []
    control_root = Path(tempfile.mkdtemp(prefix="veil-wal-control-", dir=os.environ.get("VEIL_TEST_TMPDIR", "/tmp")))

    def run(argv, okay=True):
        result = subprocess.run(list(map(str, argv)), capture_output=True, text=True, timeout=30)
        if (result.returncode == 0) != okay:
            raise RuntimeError(f'unexpected exit {result.returncode}: {result.stderr[:500]}')
        return result

    # Existing provisioning negative tests also prove that invalid TLS fails
    # before WAL creation. Fixture public metadata only is read below.
    run([sys.executable, Path(__file__).with_name('tls-pki-check.py'),
         '--binary', binary, '--output-dir', root / 'pki'])
    fixture = root / 'pki'
    key = fixture / 'consensus.pk8'
    permit = fixture / 'consensus.pk8.bootstrap'
    public = permit.read_text().splitlines()[2]
    base = [binary, '--node-id', 'a', '--listen', '127.0.0.1:5100',
            '--cluster-id', 'pki-check', '--config-epoch', '1',
            '--member', f'a@127.0.0.1:5100={public}', '--key-file', key]
    base += ['--approval-public-key', '1122334455667788112233445566778811223344556677881122334455667788', '--scope', '192.0.2.10/tcp/443']
    wal = root / 'state/node.wal'
    normal = [*base, '--tls-dir', fixture / 'tls', '--wal-file', wal]
    key_hash = hashlib.sha256(key.read_bytes()).hexdigest()
    initial = permit.read_bytes()

    out = run([*normal, '--control-socket', root / 'missing.sock',
               '--ebpf-obj', root / 'does-not-exist.o'], False)
    assert 'Existing WAL required' in out.stderr
    assert not wal.parent.exists() and not (root / 'missing.sock').exists()
    assert permit.read_bytes() == initial
    checks.append('normal startup with missing WAL fails before XDP/control/state creation')
    run([binary, '--init-key', '--node-id', 'a', '--key-file', key], False)
    assert permit.read_bytes() == initial
    checks.append('init-key cannot reissue initial-state permission for existing identity')

    sentinel = root / 'retained-existing.wal'
    sentinel.write_bytes(b'existing evidence')
    sentinel.chmod(0o600)
    run([*base, '--init-state', '--wal-file', sentinel], False)
    assert sentinel.read_bytes() == b'existing evidence' and permit.read_bytes() == initial
    checks.append('init-state refuses an existing WAL without consuming permission')
    run([*base, '--init-state', '--wal-file', wal])
    assert wal.stat().st_size > 0 and permit.read_bytes().endswith(b'CLAIMED\n')
    claimed = permit.read_bytes()
    checks.append('explicit initial WAL succeeds once without listeners or XDP')
    run([*base, '--init-state', '--wal-file', root / 'second/node.wal'], False)
    assert not (root / 'second').exists()
    checks.append('same identity cannot bootstrap another state path')

    def launch_and_stop(label):
        sock = control_root / f'{label}.sock'
        with (root / f'{label}.stdout').open('w') as stdout, (root / f'{label}.stderr').open('w') as stderr:
            child = subprocess.Popen(list(map(str, [*normal, '--control-socket', sock])), stdout=stdout, stderr=stderr)
        try:
            deadline = time.monotonic() + 12
            while time.monotonic() < deadline:
                if child.poll() is not None:
                    raise RuntimeError(f'daemon exited: {(root / f"{label}.stderr").read_text()[:500]}')
                if sock.exists():
                    status = run([binary, '--status-local', '--control-socket', sock]).stdout
                    if 'role=Leader' in status and re.search(r'commit_index=[1-9]', status):
                        # Even a second WAL pathname cannot use the locked key.
                        alternate = root / f'{label}-alternate.wal'
                        alternate.write_bytes(wal.read_bytes())
                        alternate.chmod(0o600)
                        refused = run([*base, '--tls-dir', fixture / 'tls', '--wal-file', alternate,
                                       '--control-socket', root / f'{label}-duplicate.sock'], False)
                        assert 'already in use' in refused.stderr
                        assert not (root / f'{label}-duplicate.sock').exists()
                        return int(re.search(r'^term=(\d+)$', status, re.M).group(1))
                time.sleep(0.1)
            raise RuntimeError('daemon did not elect/commit')
        finally:
            if child.poll() is None:
                child.send_signal(signal.SIGINT)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)

    term = launch_and_stop('first')
    checks.append('actual daemon elects/commits and locks signing identity across WAL paths')
    saved = wal.read_bytes()
    retained = root / 'retained-after-vote.wal'
    wal.rename(retained)  # retain acknowledged state; no deletion
    out = run([*normal, '--control-socket', root / 'lost.sock'], False)
    assert 'Existing WAL required' in out.stderr
    assert not wal.exists() and not (root / 'lost.sock').exists()
    run([*base, '--init-state', '--wal-file', wal], False)
    assert not wal.exists() and retained.read_bytes() == saved
    assert permit.read_bytes() == claimed
    checks.append('after a durable vote, lost WAL cannot restart or be reinitialized')
    for label, data in [('empty', b''), ('corrupt', b'not a WAL')]:
        bad = root / f'{label}.wal'
        bad.write_bytes(data)
        bad.chmod(0o600)
        run([*base, '--tls-dir', fixture / 'tls', '--wal-file', bad,
             '--control-socket', root / f'{label}.sock'], False)
        assert bad.read_bytes() == data and not (root / f'{label}.sock').exists()
    checks.append('empty/corrupted WALs are preserved and cannot start voting')
    retained.rename(wal)  # exact retained state, destination is absent
    assert wal.read_bytes() == saved
    assert launch_and_stop('restored') > term
    checks.append('exact retained WAL restores progress using unchanged signing key')
    assert hashlib.sha256(key.read_bytes()).hexdigest() == key_hash
    assert permit.read_bytes() == claimed
    (root / 'result.json').write_text(json.dumps({'success': True, 'checks': checks,
                                                'scope': 'loopback daemon; no XDP attachment',
                                                'control_directory': str(control_root)}, indent=2) + '\n')
    for check in checks:
        print(f'PASS: {check}', flush=True)


if __name__ == '__main__':
    main()
