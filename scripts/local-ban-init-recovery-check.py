#!/usr/bin/env python3
"""Isolated Linux acceptance for initialization SIGKILL and manual recovery.
Preserves broken directories and unknown paths. No forced cleanup or host config.
The ignored Rust test executable shares startup code but is not a release binary.
"""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import ssl
import struct
import subprocess
import sys
import tempfile
import time

NAMES = ('control.sock', 'emergency.sock', '.control.owned.sock',
         '.emergency.owned.sock', 'socket-owner', '.socket-owner.next')
STAGES = ('lease_acquired', 'normal_bound', 'normal_anchored', 'emergency_bound',
          'emergency_anchored', 'next_created', 'next_written', 'proof_published',
          'xdp_attached')
AUTO = {'lease_acquired', 'proof_published', 'xdp_attached'}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--fixture-binary', required=True)
    parser.add_argument('--object', required=True)
    parser.add_argument('--artifacts-parent', required=True)
    args = parser.parse_args()
    assert os.geteuid() == 0, 'root is required for this dedicated namespace'
    os.umask(0o077)
    root = Path(tempfile.mkdtemp(prefix='init-recovery-', dir=args.artifacts_parent))
    ns = ['ip', 'netns', 'exec', 'ns-veil-rogue']
    children, logs, foreign_sockets = [], [], []
    evidence = {'artifact_dir': str(root), 'checks': [], 'cases': {},
                'binary_sha256': hashlib.sha256(Path(args.binary).read_bytes()).hexdigest(),
                'fixture_sha256': hashlib.sha256(Path(args.fixture_binary).read_bytes()).hexdigest(),
                'object_sha256': hashlib.sha256(Path(args.object).read_bytes()).hexdigest()}

    def save():
        (root/'result.json').write_text(json.dumps(evidence, indent=2))

    def check(name, condition, **details):
        evidence['checks'].append(dict(name=name, passed=bool(condition), **details))
        save()
        print(('PASS ' if condition else 'FAIL ')+name, flush=True)
        assert condition, (name, details)

    def link():
        return json.loads(subprocess.check_output(
            ['ip', '-j', '-details', '-n', 'ns-veil-rogue', 'link', 'show', 'veth-rogue']))[0]

    def attached():
        return bool(link().get('xdp', {}).get('attached'))

    def wait(predicate, seconds=8):
        until = time.monotonic()+seconds
        while time.monotonic() < until:
            if predicate():
                return True
            time.sleep(.03)
        return False

    def spawn(argv, logfile, env=None):
        output = open(logfile, 'wb')
        logs.append(output)
        child = subprocess.Popen(argv, stdout=output, stderr=subprocess.STDOUT, env=env)
        children.append(child)
        return child

    def command(directory):
        return ns+[args.binary, 'serve', '--iface', 'veth-rogue', '--ebpf-object', args.object,
                   '--control-dir', str(directory), '--destination', '10.99.0.66', '--port', '18443']

    def request(directory, op=0, instance=None, revision=0, ttl=0):
        frame = struct.pack('>4sB3x16sQII', b'VLB1', op,
                            bytes.fromhex(instance) if instance else bytes(16), revision,
                            int.from_bytes(socket.inet_aton('10.99.0.1'), 'big'), ttl)
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(2.5)
            connection.connect(str(directory/'control.sock'))
            connection.sendall(frame)
            connection.shutdown(socket.SHUT_WR)
            data = b''
            while len(data) <= 4096:
                chunk = connection.recv(4097-len(data))
                if not chunk:
                    break
                data += chunk
        lines = data.decode().splitlines()
        assert lines and len(data) <= 4096
        return dict(ok=lines[0] == 'OK', **dict(x.split('=', 1) for x in lines[1:] if '=' in x))

    def ready(directory):
        try:
            return request(directory).get('mode') == 'local-ephemeral'
        except (OSError, AssertionError):
            return False

    def inspect(directory, destination):
        result = subprocess.run([args.binary, 'inspect-runtime', '--control-dir', str(directory)],
                                capture_output=True, text=True, timeout=4)
        destination.write_text(result.stdout+result.stderr)
        assert result.returncode == 0, result.stderr
        lines = result.stdout.splitlines()
        assert lines[0] == 'INSPECTED'
        fields = dict(x.split('=', 1) for x in lines[1:] if '=' in x)
        assert fields['recovery_attempted'] == 'false' and fields['socket_liveness'] == 'not_probed'
        return fields

    def snapshot(directory):
        result = {}
        for name in sorted(os.listdir(directory)):
            path = directory/name
            m = path.lstat()
            entry = dict(dev=m.st_dev, ino=m.st_ino, mode=m.st_mode, uid=m.st_uid,
                         links=m.st_nlink, size=m.st_size, mtime_ns=m.st_mtime_ns,
                         ctime_ns=m.st_ctime_ns)
            if path.is_symlink():
                entry['link_target'] = os.readlink(path)
            elif __import__('stat').S_ISREG(m.st_mode):
                # Only our small synthetic control-dir fixtures; never keys or
                # application data outside the test directory, no symlink following.
                assert m.st_size <= 4096
                fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                with os.fdopen(fd, 'rb') as stream:
                    entry['sha256'] = hashlib.sha256(stream.read(4097)).hexdigest()
            result[name] = entry
        return result

    def start(directory, logfile):
        environment = os.environ.copy()
        # The production executable must not contain/obey test-only stop hooks.
        environment.update(VEIL_LOCAL_BAN_INIT_FIXTURE='1', VEIL_LOCAL_BAN_INIT_STAGE='normal_bound')
        child = spawn(command(directory), logfile, environment)
        assert wait(lambda: ready(directory)), (directory, child.poll(), logfile.read_text())
        assert child.poll() is None and attached()
        return child

    def stop(child):
        if child.poll() is None:
            child.send_signal(signal.SIGCONT)
            child.terminate()
            child.wait(timeout=8)
        assert child.returncode == 0, child.returncode
        assert not attached()

    def kill(child):
        child.kill()
        child.wait(timeout=5)
        assert child.returncode == -signal.SIGKILL
        assert not attached()

    def refused(directory, logfile, expected):
        result = subprocess.run(command(directory), capture_output=True, text=True, timeout=5)
        logfile.write_text(result.stdout+result.stderr)
        assert result.returncode != 0 and expected in result.stderr, result.stderr

    cert, key = root/'cert.pem', root/'test-key.pem'
    subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
                    '-subj', '/CN=isolated-init-recovery', '-addext', 'subjectAltName=IP:10.99.0.66',
                    '-keyout', str(key), '-out', str(cert)], check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    token = os.urandom(8).hex()
    context = ssl.create_default_context(cafile=str(cert))

    def https():
        try:
            connection = http.client.HTTPSConnection('10.99.0.66', 18443, context=context,
                                                   timeout=1, source_address=('10.99.0.1', 0))
            try:
                connection.request('GET', '/')
                response = connection.getresponse()
                return response.status == 200 and response.read() == token.encode()
            finally:
                connection.close()
        except (OSError, http.client.HTTPException):
            return False

    def manual(directory, case, before):
        # Exactly one deliberately selected recovery directory, never silently
        # rotate names or modify the broken directory. mkdir must be exclusive.
        recovery = case/'recovery'
        recovery.mkdir(mode=0o700)
        child = start(recovery, case/'manual-recovery.log')
        state = request(recovery)
        check(case.name+'_manual_empty_and_https', state['occupied'] == '0'
              and state['capacity'] == '256' and https())
        check(case.name+'_original_preserved_during_recovery', snapshot(directory) == before)
        stop(child)
        check(case.name+'_original_preserved_after_recovery', snapshot(directory) == before)

    try:
        evidence['link_before'] = link()
        check('no_existing_xdp', not attached())
        server_script = str(Path(__file__).with_name('local-ban-check.py'))
        server = spawn(ns+[sys.executable, server_script, 'server', str(cert), str(key), token], root/'https.log')
        check('https_baseline', wait(https, 5))
        for stage in STAGES:
            case = root/stage
            case.mkdir(mode=0o700)
            directory = case/'broken'
            directory.mkdir(mode=0o700)
            log = case/'fixture.log'
            environment = os.environ.copy()
            environment.update(VEIL_LOCAL_BAN_INIT_FIXTURE='1', VEIL_LOCAL_BAN_INIT_STAGE=stage,
                               VEIL_LOCAL_BAN_INIT_DIR=str(directory), VEIL_LOCAL_BAN_INIT_OBJECT=args.object)
            fixture = spawn(ns+[args.fixture_binary, '--exact',
                                'local_ban::runtime::tests::initialization_crash_fixture',
                                '--ignored', '--nocapture'], log, environment)
            def stopped():
                status = Path(f'/proc/{fixture.pid}/status')
                return (status.exists() and '\nState:\tT' in status.read_text()
                        and f'VEIL_LOCAL_BAN_INIT_STAGE={stage}' in log.read_text())
            check(stage+'_stopped_at_checkpoint', wait(stopped, 8), pid=fixture.pid)
            check(stage+'_xdp_stage_matches', attached() == (stage == 'xdp_attached'))
            kill(fixture)
            before = snapshot(directory)
            details = inspect(directory, case/'inspection.txt')
            check(stage+'_inspection_does_not_change_files', snapshot(directory) == before)
            check(stage+'_lease_released', details['owner_lock'] == 'available')
            evidence['cases'][stage] = dict(automatic=stage in AUTO, before=before, inspection=details)
            if stage in AUTO:
                child = start(directory, case/'automatic-restart.log')
                check(stage+'_automatic_restart_empty', request(directory)['occupied'] == '0' and https())
                stop(child)
            else:
                expected = 'INCOMPLETE_SOCKET_PROOF_PRESERVED' if stage.startswith('next_') else 'UNOWNED_SOCKET_PATH_PRESERVED'
                refused(directory, case/'refused.log', expected)
                check(stage+'_refusal_preserves_every_entry', snapshot(directory) == before and not attached())
                manual(directory, case, before)
        for damage in ('checksum', 'truncated', 'pending'):
            case = root/('corrupt_'+damage)
            case.mkdir(mode=0o700)
            directory = case/'broken'
            directory.mkdir(mode=0o700)
            child = start(directory, case/'initial-daemon.log')
            kill(child)
            proof = directory/'socket-owner'
            original = proof.read_bytes()
            (case/'original-proof.bin').write_bytes(original)
            if damage == 'checksum':
                changed = bytearray(original)
                changed[-1] ^= 1
                proof.write_bytes(changed)
            elif damage == 'truncated':
                proof.write_bytes(original[:17])
            else:
                (directory/'.socket-owner.next').write_bytes(b'partial fixture metadata')
            (directory/'keep.txt').write_bytes(b'unknown user file: preserve exactly')
            before = snapshot(directory)
            details = inspect(directory, case/'inspection.txt')
            expected = {'checksum': 'INVALID_SOCKET_PROOF', 'truncated': 'UNRECOGNIZED_SOCKET_PROOF',
                        'pending': 'INCOMPLETE_SOCKET_PROOF_PRESERVED'}[damage]
            refused(directory, case/'refused.log', expected)
            check(damage+'_corruption_preserved', snapshot(directory) == before and not attached())
            manual(directory, case, before)
            evidence['cases']['corrupt_'+damage] = dict(before=before, inspection=details)
        # A listener in another process (this driver, not the daemon) must still
        # work throughout the operator's switch to an explicitly empty directory.
        case = root/'foreign_listener'
        case.mkdir(mode=0o700)
        directory = case/'broken'
        directory.mkdir(mode=0o700)
        foreign = socket.socket(socket.AF_UNIX)
        foreign.bind(str(directory/'control.sock'))
        foreign.listen(8)
        foreign_sockets.append(foreign)
        (directory/'keep.txt').write_bytes(b'foreign process data')
        before = snapshot(directory)
        inspect(directory, case/'inspection.txt')
        refused(directory, case/'refused.log', 'UNOWNED_SOCKET_PATH_PRESERVED')
        manual(directory, case, before)
        with socket.socket(socket.AF_UNIX) as probe:
            probe.settimeout(1)
            probe.connect(str(directory/'control.sock'))
        check('foreign_listener_still_connectable', snapshot(directory) == before)
        # Backstop against an operator selecting a second control directory
        # while a known original daemon still owns the same XDP interface.
        case = root/'live_owner_backstop'
        case.mkdir(mode=0o700)
        directory = case/'broken'
        directory.mkdir(mode=0o700)
        child = start(directory, case/'live-daemon.log')
        state = request(directory)
        proof = directory/'socket-owner'
        proof.write_bytes(b'corrupted while running fixture')
        before = snapshot(directory)
        details = inspect(directory, case/'inspection.txt')
        check('live_corruption_reports_busy', details['owner_lock'] == 'busy' and details['owner_record'] == 'invalid')
        refused(directory, case/'same-directory-refused.log', 'CONTROL_DIRECTORY_BUSY')
        alternate = case/'recovery'
        alternate.mkdir(mode=0o700)
        xdp_before = link().get('xdp')
        refused(alternate, case/'alternate-refused.log', 'Device or resource busy')
        check('alternate_cannot_replace_active_xdp', link().get('xdp') == xdp_before
              and request(directory)['instance'] == state['instance'] and https())
        check('live_corrupt_original_unchanged', snapshot(directory) == before)
        kill(child)
        check('postkill_corruption_still_preserved', snapshot(directory) == before and https())
        evidence['complete'] = True
    except BaseException as error:
        evidence['complete'] = False
        evidence['error'] = repr(error)
        raise
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.send_signal(signal.SIGCONT)
                child.terminate()
                try:
                    child.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
        for foreign in foreign_sockets:
            foreign.close()  # Keep the pathname as preserved evidence.
        for output in logs:
            output.close()
        evidence['link_after'] = link()
        evidence['cleanup_ok'] = not attached() and all(c.poll() is not None for c in children)
        save()
        print('ARTIFACTS='+str(root), flush=True)


if __name__ == '__main__':
    main()
