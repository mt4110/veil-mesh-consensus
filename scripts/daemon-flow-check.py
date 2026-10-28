#!/usr/bin/env python3
"""Black-box Linux daemon acceptance using only normal CLI/process/network paths.

Creates a fresh private evidence directory and retains it. Stops only children
it starts. --xdp uses the existing veth-victim/ns-veil-rogue fixture, never a
physical interface, and refuses an existing XDP attachment.
"""
import argparse
import json
import hashlib
import sys
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import time
import struct
from collections import deque


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--output-dir', required=True, type=Path)
    parser.add_argument('--xdp', type=Path, help='BPF object; requires root and existing isolated fixture')
    parser.add_argument('--checkpoints', action='store_true', help='Exercise compaction, offline state transfer and restart with real daemon traffic')
    args = parser.parse_args()
    binary = str(args.binary.resolve())
    root = args.output_dir.resolve()
    root.mkdir(mode=0o700)  # never overwrite a previous run
    report = {'binary': binary, 'evidence': str(root), 'xdp': bool(args.xdp), 'phases': []}
    children, controls, generations = {}, {}, {}
    reserved = []
    tls_dirs = {}
    tls_names = {}
    provisioner = Path(__file__).with_name('tls-pki.py')
    names = ['a', 'b', 'c', 'd']
    source = '10.99.0.66'
    service_ports = [18443, 18444]
    source_mac = destination_mac = None
    approval_number = 0
    operator_key = root / 'operator.pk8'
    profile = []
    wait_samples = deque(maxlen=32)


    def run(argv, timeout=8):
        return subprocess.run([str(x) for x in argv], capture_output=True, text=True, timeout=timeout)

    def require(ok, message):
        if not ok:
            raise RuntimeError(message)

    def probe(protocol='udp', outside=False, destination='10.99.0.1'):
        # Observe real ingress after XDP and before the host's UFW policy.
        # This tests packet filtering, not an HTTP/TCP application handshake.
        port = service_ports[1 if outside else 0]
        nonce = os.urandom(32)
        src, dst = socket.inet_aton(source), socket.inet_aton(destination)
        def checksum(data):
            if len(data) % 2:
                data += b'\0'
            total = sum(struct.unpack('!' + 'H' * (len(data) // 2), data))
            while total >> 16:
                total = (total & 65535) + (total >> 16)
            return (~total) & 65535
        number = 17 if protocol == 'udp' else 6
        if protocol == 'udp':
            segment = struct.pack('!HHHH', 49123, port, 8 + len(nonce), 0) + nonce
        else:
            segment = struct.pack('!HHIIBBHHH', 49123, port, 1, 0, 0x50, 2, 4096, 0, 0) + nonce
            check = checksum(src + dst + struct.pack('!BBH', 0, 6, len(segment)) + segment)
            segment = segment[:16] + struct.pack('!H', check) + segment[18:]
        header = struct.pack('!BBHHHBBH4s4s', 0x45, 0, 20 + len(segment), 123, 0, 64, number, 0, src, dst)
        header = header[:10] + struct.pack('!H', checksum(header)) + header[12:]
        frame = destination_mac + source_mac + b'\x08\x00' + header + segment
        with socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(3)) as capture:
            capture.bind(('veth-victim', 0)); capture.settimeout(0.4)
            result = run(['ip', 'netns', 'exec', 'ns-veil-rogue', sys.executable, '-c',
                          "import socket,sys;s=socket.socket(socket.AF_PACKET,socket.SOCK_RAW);s.bind(('veth-rogue',0));s.send(bytes.fromhex(sys.argv[1]))", frame.hex()])
            require(result.returncode == 0, f'packet injection failed: {result.stderr}')
            end = time.monotonic() + 0.4
            while time.monotonic() < end:
                capture.settimeout(max(0.001, end - time.monotonic()))
                try:
                    packet = capture.recv(2048)
                    if packet == frame:
                        return True
                except socket.timeout:
                    break
        return False

    def ping():
        return probe()

    def no_attachment():
        result = run(['ip', '-details', 'link', 'show', 'veth-victim'])
        require(result.returncode == 0, 'isolated veth fixture missing')
        return 'prog/xdp' not in result.stdout

    def cli(name, *argv):
        return run([binary, '--control-socket', controls[name], *argv])

    def status(name, index=None):
        argv = ['--status-local', '--status-ip', source]
        if index is not None:
            argv += ['--entry-index', str(index)]
        result = cli(name, *argv)
        require(result.returncode == 0, f'{name} status failed: {result.stderr}')
        return dict(line.split('=', 1) for line in result.stdout.splitlines() if '=' in line)

    def wait_for(description, predicate, seconds=20):
        wait_samples.clear()
        started = time.monotonic()
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            for name, proc in children.items():
                require(proc.poll() is None, f'{name} unexpectedly exited; inspect logs')
            result = predicate()
            if result:
                report.setdefault('completed_waits', []).append({
                    'description': description, 'limit_secs': seconds,
                    'elapsed_secs': time.monotonic() - started})
                return result
            time.sleep(0.1)
        report['failed_wait'] = {'description': description, 'limit_secs': seconds,
                                 'elapsed_secs': time.monotonic() - started,
                                 'samples': list(wait_samples)}
        raise RuntimeError(f'timed out: {description}')

    def stop(name):
        proc = children.pop(name)
        proc.send_signal(signal.SIGCONT)
        proc.send_signal(signal.SIGINT)
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)

    def phase(name, **data):
        report['phases'].append({'name': name, **data})
        print(f'{name}: PASS', flush=True)
        (root / 'result.json').write_text(json.dumps(report, indent=2) + '\n')

    try:
        if args.xdp:
            require(os.geteuid() == 0, '--xdp requires root')
            require(no_attachment(), 'fixture already has XDP; refusing replacement')
            host_link = json.loads(run(['ip', '-j', 'link', 'show', 'veth-victim']).stdout)[0]
            remote_links = json.loads(run(['ip', '-n', 'ns-veil-rogue', '-j', 'address', 'show', 'veth-rogue']).stdout)
            require(any(a.get('local') == source for a in remote_links[0]['addr_info']), 'unexpected fixture source')
            destination_mac = bytes.fromhex(host_link['address'].replace(':', ''))
            source_mac = bytes.fromhex(remote_links[0]['address'].replace(':', ''))
            report['packet_observation'] = 'real veth ingress after XDP, before UFW; no application handshake'
            require(ping() and probe('tcp'), 'baseline fixture TCP/UDP ingress failed')
        op = run([binary, '--init-approver', '--approval-key-file', operator_key])
        require(op.returncode == 0, f'operator provisioning failed: {op.stderr}')
        operator_public = re.search(r'^approver ([0-9a-f]{64})$', op.stdout, re.M).group(1)
        profile = ['--approval-public-key', operator_public,
                   '--scope', f'10.99.0.1/tcp/{service_ports[0]}', '--scope', f'10.99.0.1/udp/{service_ports[0]}']
        members, addresses = [], {}
        for name in names:
            result = run([binary, '--init-key', '--node-id', name, '--key-file', root / f'{name}.pk8'])
            require(result.returncode == 0, f'key provisioning failed: {result.stderr}')
            match = re.search(rf'^{name} ([0-9a-f]{{64}})$', result.stdout, re.M)
            require(match is not None, 'missing public key output')
            reserve = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            reserve.bind(('127.0.0.1', 0))
            reserved.append(reserve)
            addresses[name] = f'127.0.0.1:{reserve.getsockname()[1]}'
            members += ['--member', f'{name}@{addresses[name]}={match.group(1)}']
        def provision(*argv):
            result = run([sys.executable, provisioner, *argv], timeout=30)
            require(result.returncode == 0, f'TLS provisioning failed: {result.stderr}')

        provision('init-ca', '--out', root / 'offline-ca')
        for name in names:
            result = run([binary, '--node-id', name, '--listen', addresses[name],
                          '--cluster-id', 'daemon-flow-fixture', '--config-epoch', '1',
                          '--print-tls-name', *members, *profile])
            require(result.returncode == 0, f'TLS name failed: {result.stderr}')
            tls_names[name] = result.stdout.strip()
            provision('request', '--out', root / f'{name}-request')
            provision('issue', '--ca-dir', root / 'offline-ca', '--csr', root / f'{name}-request/node.csr.der',
                      '--tls-name', tls_names[name], '--out', root / f'{name}-issued')
            tls_dirs[name] = root / f'{name}-tls'
            provision('bundle', '--request-dir', root / f'{name}-request', '--issued-dir', root / f'{name}-issued',
                      '--revoked', root / f'{name}-issued/revoked.txt', '--out', tls_dirs[name])
            require(set(p.name for p in tls_dirs[name].iterdir()) == {'node.key.der', 'node.cert.der', 'ca.cert.der', 'revoked.txt'},
                    'daemon bundle contains extra material')
        for name in names:
            result = run([binary, '--init-state', '--node-id', name, '--listen', addresses[name],
                          '--cluster-id', 'daemon-flow-fixture', '--config-epoch', '1',
                          '--key-file', root / f'{name}.pk8', '--wal-file', root / f'{name}.wal', *members, *profile])
            require(result.returncode == 0, f'initial state failed: {result.stderr}')
        report['tls_certificate_sha256'] = {n: hashlib.sha256((tls_dirs[n] / 'node.cert.der').read_bytes()).hexdigest() for n in names}
        require(len(set(report['tls_certificate_sha256'].values())) == 4, 'TLS certificates are not distinct')
        report['daemon_bundles_have_no_ca_private_key'] = True
        def start(name):
            generation = generations.get(name, 0) + 1
            generations[name] = generation
            controls[name] = root / f'{name}-{generation}.sock'
            argv = [binary, '--node-id', name, '--listen', addresses[name],
                    '--cluster-id', 'daemon-flow-fixture', '--config-epoch', '1',
                    '--cluster-dir', root / 'state', '--tls-dir', tls_dirs[name], '--key-file', root / f'{name}.pk8',
                    '--wal-file', root / f'{name}.wal', '--control-socket', controls[name], *members, *profile]
            if args.xdp and name == 'd':
                argv += ['--iface', 'veth-victim', '--ebpf-obj', args.xdp.resolve()]
            with (root / f'{name}-{generation}.stdout').open('w') as out, (root / f'{name}-{generation}.stderr').open('w') as err:
                children[name] = subprocess.Popen([str(x) for x in argv], stdout=out, stderr=err)
        for sock in reserved:
            sock.close()
        for name in names:
            start(name)
        wait_for('all control sockets', lambda: all(p.exists() for p in controls.values()))

        def elected(active):
            states = {name: status(name) for name in active}
            leaders = [name for name, s in states.items() if s['role'] == 'Leader']
            if len(leaders) != 1:
                return None
            leader = leaders[0]
            s = states[leader]
            if int(s['commit_index']) == 0 or s['entry_phase'] not in ('applied', 'compacted'):
                return None
            if not all(n['leader'] == leader and n['term'] == s['term'] for n in states.values()):
                return None
            return leader

        leader = wait_for('automatic election and NoOp commit', lambda: elected(names))
        phase('startup_and_automatic_election', leader=leader, statuses={n: status(n) for n in names})
        follower = next(n for n in names if n != leader)
        rejected = cli(follower, '--ban-ip', source, '--ttl-secs', '8')
        require(rejected.returncode != 0 and 'not leader' in rejected.stderr, 'follower accepted BAN')
        before = status(leader)['last_log_index']
        rejected = cli(leader, '--ban-ip', source, '--ttl-secs', '8')
        require(rejected.returncode != 0 and 'approval' in rejected.stderr.lower(), 'raw BAN accepted without operator approval')
        require(status(leader)['last_log_index'] == before, 'rejected BAN changed log')
        phase('follower_and_unapproved_input_rejected')

        def make_approval(name, ttl, unban=False, ip=source):
            nonlocal approval_number
            approval_number += 1
            path = root / f'approval-{approval_number}.bin'
            revision = int(status(name)['authorization_revision'])
            argv = [binary, '--node-id', name, '--listen', addresses[name],
                    '--cluster-id', 'daemon-flow-fixture', '--config-epoch', '1',
                    *members, *profile, '--approval-key-file', operator_key,
                    '--expected-policy-revision', str(revision), '--approval-output', path,
                    '--approve-unban-ip' if unban else '--approve-ban-ip', ip]
            if not unban:
                argv += ['--ttl-secs', str(ttl)]
            result = run(argv)
            require(result.returncode == 0, f'operator approval failed: {result.stderr}')
            deadline = int(re.search(r'not_after_unix_ns=(\d+)', result.stdout).group(1))
            return path, deadline

        rejected = run([binary, '--node-id', leader, '--listen', addresses[leader],
                        '--cluster-id', 'daemon-flow-fixture', '--config-epoch', '1',
                        *members, *profile, '--approval-key-file', operator_key,
                        '--expected-policy-revision', status(leader)['authorization_revision'],
                        '--approval-output', root / 'protected-rejected.bin',
                        '--approve-ban-ip', '127.0.0.2', '--ttl-secs', '30'])
        require(rejected.returncode != 0 and 'loopback' in rejected.stderr.lower(),
                'operator could approve protected loopback')
        require(not (root / 'protected-rejected.bin').exists(), 'invalid approval was written')
        phase('protected_ip_rejected_even_with_operator_key')

        def ban(name, ttl, unban=False):
            path, deadline = make_approval(name, ttl, unban)
            result = cli(name, '--submit-approval', path)
            if result.returncode != 0:
                report['rejected_approval'] = {'node': name, 'deadline': deadline,
                    'observed_unix_ns': time.time_ns(), 'error': result.stderr,
                    'statuses': {n: status(n) for n in children}}
            require(result.returncode == 0, f'approval submission failed: {result.stderr}')
            request_id = re.search(r'^REQUESTED id=([0-9a-f]{64})$', result.stdout, re.M).group(1)
            def logged():
                started = time.monotonic_ns()
                response = cli(name, '--status-local', '--request-id', request_id)
                require(response.returncode == 0, 'request status failed')
                fields = dict(line.split('=', 1) for line in response.stdout.splitlines() if '=' in line)
                wait_samples.append({'request_id': request_id, 'node': name, 'deadline': deadline,
                    'observed_unix_ns': time.time_ns(), 'duration_ns': time.monotonic_ns() - started,
                    'request_status': fields})
                return fields if int(fields.get('entry_index', 0)) > 0 else None
            entry = wait_for('approved operation logged', logged, 5)
            st = status(name, int(entry['entry_index']))
            return {'index': int(entry['entry_index']), 'term': int(st['term']), 'hash': st['entry_hash'],
                    'deadline': deadline, 'approval': str(path), 'request_id': request_id}

        def applied(active, receipt, verdict=None):
            started = time.monotonic_ns()
            sampled_unix_ns = time.time_ns()
            states = {n: status(n, receipt['index']) for n in active}
            wait_samples.append({'started_monotonic_ns': started,
                                 'sampled_unix_ns': sampled_unix_ns,
                                 'duration_ns': time.monotonic_ns() - started,
                                 'deadline_remaining_ns': receipt['deadline'] - sampled_unix_ns,
                                 'receipt': receipt, 'expected_verdict': verdict,
                                 'states': states})
            # A boundary receipt is retained as the certified checkpoint hash.
            # Earlier compacted receipts lack an individual hash and still fail
            # this equality; compaction alone is not evidence of this command.
            ok = all(s['entry_phase'] in ('applied', 'compacted') and s['entry_hash'] == receipt['hash']
                     and int(s['last_applied']) >= receipt['index']
                     and int(s['commit_index']) >= receipt['index']
                     and s['persistence_error'] == 'None' and s['apply_error'] == 'None'
                     and (verdict is None or s['policy_mirror'] == verdict) for s in states.values())
            return states if ok else None

        receipt = ban(leader, 8)
        statuses = wait_for('all four applied identical BAN', lambda: applied(names, receipt, 'Drop'), 5)
        if args.xdp:
            require(not ping(), 'BAN did not drop fixture packet')
            dropped = status('d', receipt['index'])
            require(int(dropped['kernel_drop']) > 0, 'no kernel DROP evidence')
            children['d'].send_signal(signal.SIGSTOP)
        # Kernel expiry must not depend on a daemon tick; the entire XDP-owning
        # process stays stopped through the signed deadline.
        time.sleep(max(0, (receipt['deadline'] - time.time_ns()) / 1e9) + 0.2)
        if args.xdp:
            require(ping(), 'expired BAN blocked while daemon SIGSTOP')
            children['d'].send_signal(signal.SIGCONT)
        statuses = wait_for('all mirrors expire', lambda: applied(names, receipt, 'Pass'))
        phase('ban_commit_apply_and_expiry', receipt=receipt, statuses=statuses,
              kernel_drop_then_pass_while_sigstop=bool(args.xdp))

        # The resilient path must work through an ordinary follower's owner socket.
        current = wait_for('leader before witnessed request', lambda: elected(names))
        witness = next(n for n in names if n != current)
        requested = ban(witness, 8)
        request_id = requested['request_id']
        wait_for('witnessed BAN on four nodes', lambda: applied(names, requested, 'Drop'), 5)
        if args.xdp:
            require(not ping(), 'witnessed BAN did not drop fixture packet')
        time.sleep(max(0, (requested['deadline'] - time.time_ns()) / 1e9) + 0.2)
        if args.xdp:
            require(ping(), 'witnessed BAN did not expire')
        statuses = wait_for('witnessed BAN expired on all mirrors', lambda: applied(names, requested, 'Pass'))
        phase('follower_witnessed_request_commit_apply_expiry', witness=witness,
              request_id=request_id, receipt=requested, statuses=statuses)

        # SIGSTOP/resume can itself cause a new election. Re-observe the actual
        # current leader before stopping it; never assume the initial one remains.
        leader = wait_for('stable current leader before stop', lambda: elected(names))
        before_stop = status(leader)
        require(before_stop['role'] == 'Leader', 'stop target ceased to be leader')
        # Ordinary process stop/restart; not an adversarial BFT view-change proof.
        stop(leader)
        remaining = [n for n in names if n != leader]
        new_leader = wait_for('replacement leader with 3 of 4', lambda: elected(remaining))
        after_stop = status(new_leader)
        require(new_leader != leader and int(after_stop['term']) > int(before_stop['term']),
                'replacement election did not advance the stopped leader term')
        receipt2 = ban(new_leader, 8)
        wait_for('replacement leader commits', lambda: applied(remaining, receipt2, 'Drop'), 5)
        start(leader)
        wait_for('restarted control socket', lambda: controls[leader].exists())
        wait_for('restarted node catches up', lambda: applied(names, receipt2, 'Drop'), 5)
        phase('leader_stop_reelection_and_wal_restart', old_leader=leader, new_leader=new_leader,
              before_stop=before_stop, after_stop=after_stop, receipt=receipt2)
        time.sleep(max(0, (receipt2['deadline'] - time.time_ns()) / 1e9) + 0.2)
        wait_for('second BAN expires', lambda: applied(names, receipt2, 'Pass'))

        def ready_for_quorum_loss():
            leader = elected(names)
            if not leader:
                return None
            states = {n: status(n) for n in names}
            # Expiry in the kernel precedes the consensus retirement of its
            # history. Partitioning between these steps strands a cleanup entry
            # at the sole pending height and cannot test a pending BAN proposal.
            return leader if all(int(s['retired_through_unix_ns']) >= receipt2['deadline']
                and s['last_log_index'] == s['commit_index'] == s['last_applied']
                for s in states.values()) else None
        leader = wait_for('stable leader and committed retirement before quorum loss', ready_for_quorum_loss)
        stopped = [n for n in names if n != leader][:2]
        for n in stopped:
            stop(n)
        active = [n for n in names if n not in stopped]
        prior_commit = status(leader)['commit_index']
        pending = ban(leader, 4)
        time.sleep(0.7)
        minority = status(leader, pending['index'])
        require(minority['entry_phase'] == 'pending' and minority['commit_index'] == prior_commit,
                'minority proposal falsely reported committed/applied')
        require(all(status(n)['policy_mirror'] == 'Pass' for n in active), 'minority BAN applied')
        time.sleep(max(0, (pending['deadline'] - time.time_ns()) / 1e9) + 0.2)
        for n in stopped:
            start(n)
        wait_for('restored control sockets', lambda: all(controls[n].exists() for n in names))
        leader = wait_for('new view after bare approval expired', lambda: elected(names))
        require(all(status(n)['policy_mirror'] == 'Pass' for n in names), 'expired bare approval activated')
        require(int(status(leader)['authorization_revision']) == int(minority['authorization_revision']),
                'expired unprepared operation advanced authorization revision')
        if args.xdp:
            require(ping(), 'expired catch-up rearmed kernel BAN')
        phase('minority_pending_and_expired_catchup', receipt=pending, minority=minority,
              statuses={n: status(n) for n in names})
        # Rotate one leaf/key using only a new CSR. Consensus keys and WAL stay
        # in place. Distribute a denylist of the old leaf to every restarted node.
        target = 'a'
        original_consensus_key_hash = hashlib.sha256((root / f'{target}.pk8').read_bytes()).hexdigest()
        prior = status(target)
        old_tls = tls_dirs[target]
        provision('request', '--out', root / 'a-request-rotated')
        provision('issue', '--ca-dir', root / 'offline-ca', '--csr', root / 'a-request-rotated/node.csr.der',
                  '--tls-name', tls_names[target], '--out', root / 'a-issued-rotated')
        provision('revoke', '--certificate', old_tls / 'node.cert.der', '--previous', old_tls / 'revoked.txt',
                  '--out', root / 'revoked-next.txt')
        stop(target)
        for name in names:
            request_dir = root / ('a-request-rotated' if name == target else f'{name}-request')
            issued_dir = root / ('a-issued-rotated' if name == target else f'{name}-issued')
            tls_dirs[name] = root / f'{name}-tls-next'
            provision('bundle', '--request-dir', request_dir, '--issued-dir', issued_dir,
                      '--revoked', root / 'revoked-next.txt', '--out', tls_dirs[name])
        start(target)
        wait_for('rotated node ready', lambda: controls[target].exists())
        for name in names:
            if name != target:
                stop(name)
                start(name)
                wait_for('updated node ready', lambda: controls[name].exists())
        leader = wait_for('quorum after TLS rotation and revocation', lambda: elected(names))
        require(hashlib.sha256((root / f'{target}.pk8').read_bytes()).hexdigest() == original_consensus_key_hash,
                'TLS rotation modified consensus signing key')
        require(int(status(target)['commit_index']) >= int(prior['commit_index']), 'TLS rotation lost committed WAL')
        rotated_receipt = ban(leader, 4)
        wait_for('BAN commit after TLS rotation', lambda: applied(names, rotated_receipt, 'Drop'), 3)
        time.sleep(max(0, (rotated_receipt['deadline'] - time.time_ns()) / 1e9) + 0.2)
        wait_for('rotated BAN expiry', lambda: applied(names, rotated_receipt, 'Pass'))
        if args.xdp:
            require(ping(), 'rotation left an active BAN')
        phase('node_tls_rotation_and_distributed_revocation', receipt=rotated_receipt,
              old_certificate_sha256=report['tls_certificate_sha256'][target],
              new_certificate_sha256=hashlib.sha256((tls_dirs[target] / 'node.cert.der').read_bytes()).hexdigest(),
              consensus_key_unchanged=True, wal_commit_preserved=True)
        leader = wait_for('leader before authorization checks', lambda: elected(names))
        scoped = ban(leader, 30)
        wait_for('scoped BAN applied', lambda: applied(names, scoped, 'Drop'), 5)
        if args.xdp:
            require(not probe('tcp') and not probe('udp'), 'declared service escaped BAN')
            require(probe('tcp', True) and probe('udp', True), 'unrelated service was blocked')
            require(probe('tcp', destination='10.99.0.2') and probe('udp', destination='10.99.0.2'), 'unrelated destination was blocked')
            require(run(['ip', 'netns', 'exec', 'ns-veil-rogue', 'ping', '-n', '-c', '1', '-W', '1', '10.99.0.1']).returncode == 0,
                    'non-scoped ICMP was blocked')
            require(cli('d', '--bypass-local', 'on').returncode == 0 and ping(), 'bypass did not apply')
            stop('d'); start('d')
            wait_for('bypass WAL restart', lambda: controls['d'].exists())
            require(status('d')['bypass'] == 'true' and ping(), 'bypass did not survive restart')
            require(cli('d', '--bypass-local', 'off').returncode == 0 and not ping(), 'explicit bypass-off failed')
        leader = wait_for('leader before individual release', lambda: elected(names))
        released = ban(leader, 0, unban=True)
        wait_for('individual release applied', lambda: applied(names, released, 'Pass'), 5)
        if args.xdp:
            require(ping() and probe('tcp'), 'individual release did not restore service')
        require(cli(leader, '--submit-approval', scoped['approval']).returncode != 0, 'old BAN replay accepted')
        forged, _ = make_approval(leader, 30)
        damaged = root / 'tampered-approval.bin'
        raw = bytearray(forged.read_bytes()); raw[-1] ^= 1; damaged.write_bytes(raw)
        require(cli(leader, '--submit-approval', damaged).returncode != 0, 'invalid approval signature accepted')
        require(all(status(n)['policy_mirror'] == 'Pass' for n in names), 'rejected approval altered policy')
        phase('scoped_authorization_individual_unban_and_replay', ban=scoped, unban=released,
              protocols=['tcp', 'udp'], unrelated_ports_pass=True, non_scoped_icmp_pass=True)
        if args.checkpoints:
            leader = wait_for('leader before checkpoint trial', lambda: elected(names))
            late = next(n for n in names if n not in (leader, 'd'))
            stop(late)
            active = [n for n in names if n != late]
            initial = status(leader)
            # Cross two stable boundaries with one member offline. Ordinary CLI
            # receipts, the normal driver, node TLS and real XDP remain in use.
            target_height = ((int(initial['last_log_index']) // 32) + 2) * 32
            samples = []
            latest = None
            while int(status(leader)['last_log_index']) < target_height:
                latest = ban(leader, 8)
                wait_for('checkpoint workload applied', lambda: applied(active, latest, 'Drop'), 5)
                current = status(leader)
                if latest['index'] % 32 == 0:
                    wait_for('stable checkpoint on active quorum', lambda: all(int(status(n)['checkpoint_height']) >= latest['index'] for n in active))
                    samples.append({n: status(n) for n in active})
            require(latest is not None, 'checkpoint workload missing')
            if args.xdp:
                require(not ping(), 'checkpoint lost active kernel BAN')
            time.sleep(max(0, (latest['deadline'] - time.time_ns()) / 1e9) + 0.2)
            if args.xdp:
                require(ping(), 'checkpoint extended kernel deadline')
            start(late)
            wait_for('late node ready', lambda: controls[late].exists())
            wait_for('certified snapshot transfer', lambda: int(status(late)['checkpoint_height']) == target_height)
            wait_for('late node expired policy', lambda: applied(names, latest, 'Pass'))
            leader = wait_for('leader after snapshot transfer', lambda: elected(names))
            # Restart the XDP owner from the compacted WAL and retain expiry.
            stop('d'); start('d')
            wait_for('compacted XDP owner ready', lambda: controls['d'].exists())
            wait_for('compacted WAL restored', lambda: applied(names, latest, 'Pass'))
            if args.xdp:
                require(ping(), 'compacted restart resurrected BAN')
            leader = wait_for('leader after compacted restart', lambda: elected(names))
            require(cli(leader, '--submit-approval', scoped['approval']).returncode != 0, 'checkpoint forgot operator revision fence')
            resumed = ban(leader, 6)
            wait_for('post-checkpoint consensus', lambda: applied(names, resumed, 'Drop'), 5)
            if args.xdp:
                require(not ping(), 'post-checkpoint BAN not enforced')
            time.sleep(max(0, (resumed['deadline'] - time.time_ns()) / 1e9) + 0.2)
            wait_for('post-checkpoint expiry', lambda: applied(names, resumed, 'Pass'))
            phase('stable_checkpoints_state_transfer_and_restart', offline=late, target_height=target_height,
                  snapshots=samples, expired_receipt=latest, resumed_receipt=resumed,
                  statuses={n: status(n) for n in names})
        report['success'] = True
    except Exception as exc:
        report['success'] = False
        report['error'] = str(exc)
        raise
    finally:
        for sock in reserved:
            sock.close()
        for name in list(children):
            stop(name)
        if args.xdp:
            report['cleanup_no_xdp'] = no_attachment()
            report['cleanup_ping'] = ping()
            if not report['cleanup_no_xdp'] or not report['cleanup_ping']:
                report['success'] = False
        (root / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps({k: report[k] for k in ('evidence', 'success', 'cleanup_no_xdp', 'cleanup_ping') if k in report}), flush=True)
    require(report['success'], 'acceptance or cleanup failed; inspect result.json')


if __name__ == '__main__':
    main()
