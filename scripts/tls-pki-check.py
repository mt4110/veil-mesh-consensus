#!/usr/bin/env python3
"""Offline issuance + daemon startup rejection checks. No deletion or networking.
Requires an already-built veil-node and Python 3/OpenSSL >= 3.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    root = args.output_dir.resolve()
    root.mkdir(mode=0o700)
    binary = str(args.binary.resolve())
    script = Path(__file__).with_name('tls-pki.py')
    passed = []

    def run(argv, okay=True):
        result = subprocess.run(list(map(str, argv)), capture_output=True, text=True, timeout=20)
        if (result.returncode == 0) != okay:
            raise RuntimeError(f'unexpected exit {result.returncode}: {argv[0]} {result.stderr[:500]}')
        return result

    def pki(*argv, okay=True):
        return run([sys.executable, script, *argv], okay)

    public = run([binary, '--init-key', '--node-id', 'a', '--key-file', root / 'consensus.pk8']).stdout
    public = re.search(r'^a ([0-9a-f]{64})$', public, re.M).group(1)
    base = [binary, '--node-id', 'a', '--listen', '127.0.0.1:5100', '--cluster-id', 'pki-check', '--config-epoch', '1', '--member', f'a@127.0.0.1:5100={public}']
    base += ['--approval-public-key', '1122334455667788112233445566778811223344556677881122334455667788', '--scope', '192.0.2.10/tcp/443']
    name = run([*base, '--print-tls-name']).stdout.strip()
    pki('init-ca', '--out', root / 'ca')
    pki('request', '--out', root / 'request')
    pki('issue', '--ca-dir', root / 'ca', '--csr', root / 'request/node.csr.der', '--tls-name', name, '--out', root / 'issued')
    pki('bundle', '--request-dir', root / 'request', '--issued-dir', root / 'issued', '--revoked', root / 'issued/revoked.txt', '--out', root / 'tls')
    passed.append('node CSR -> offline issue -> node bundle')
    preflight = run([*base, '--check-tls', '--tls-dir', root / 'tls'])
    assert preflight.stdout.startswith('OK node=a ')
    passed.append('read-only daemon TLS preflight')
    before = hashlib.sha256((root / 'tls/node.key.der').read_bytes()).hexdigest()
    pki('bundle', '--request-dir', root / 'request', '--issued-dir', root / 'issued', '--revoked', root / 'issued/revoked.txt', '--out', root / 'tls', okay=False)
    assert hashlib.sha256((root / 'tls/node.key.der').read_bytes()).hexdigest() == before
    passed.append('existing bundle not overwritten')
    pki('request', '--out', root / 'other-request')
    pki('bundle', '--request-dir', root / 'other-request', '--issued-dir', root / 'issued', '--revoked', root / 'issued/revoked.txt', '--out', root / 'mismatch', okay=False)
    passed.append('wrong node private key rejected')
    pki('issue', '--ca-dir', root / 'ca', '--csr', root / 'request/node.csr.der', '--tls-name', '*.veil.invalid', '--out', root / 'wildcard', okay=False)
    pki('issue', '--ca-dir', root / 'ca', '--csr', root / 'request/node.csr.der', '--tls-name', name, '--days', '91', '--out', root / 'long-lived', okay=False)
    passed.append('wildcard identity and excessive validity rejected')
    pki('revoke', '--certificate', root / 'issued/node.cert.der', '--previous', root / 'issued/revoked.txt', '--out', root / 'revoked.txt')
    pki('bundle', '--request-dir', root / 'request', '--issued-dir', root / 'issued', '--revoked', root / 'revoked.txt', '--out', root / 'revoked-bundle', okay=False)
    passed.append('revoked leaf cannot be bundled')
    # Every invalid startup must stop before WAL or control creation. No daemon
    # is left running; a timeout is a test failure, never rejection evidence.
    for scenario in ('missing-tls', 'missing-key', 'wrong-context', 'revoked', 'bad-mode', 'ca-private-present'):
        trial = root / scenario
        trial.mkdir(mode=0o700)
        tls = trial / 'tls'
        tls.mkdir(mode=0o700)
        for file in (root / 'tls').iterdir():
            if scenario == 'missing-key' and file.name == 'node.key.der':
                continue
            target = tls / file.name
            target.write_bytes(file.read_bytes())
            target.chmod(0o600)
        command = [*base, '--key-file', root / 'consensus.pk8', '--wal-file', trial / 'state/node.wal', '--control-socket', trial / 'control.sock']
        if scenario != 'missing-tls':
            command += ['--tls-dir', tls]
        if scenario == 'wrong-context':
            command[command.index('pki-check')] = 'other-cluster'
        elif scenario == 'revoked':
            (tls / 'revoked.txt').write_bytes((root / 'revoked.txt').read_bytes())
        elif scenario == 'bad-mode':
            (tls / 'node.key.der').chmod(0o644)
        elif scenario == 'ca-private-present':
            (tls / 'ca.key.der').write_bytes(b'forbidden fixture; not a real CA key')
        run(command, okay=False)
        assert not (trial / 'state').exists() and not (trial / 'control.sock').exists()
        passed.append('daemon startup rejected before mutation: ' + scenario)
    result = {'success': True, 'checks': passed, 'evidence': str(root)}
    (root / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
