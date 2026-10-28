#!/usr/bin/env python3
"""Allowlisted, opt-in pi-node permanent-unit acceptance. Never enables or deletes.
Install/accept require explicit --apply; obtain human authorization separately.
Retain partial installs and evidence instead of adopting/chowning unknown paths.
"""
import argparse
import grp
import hashlib
import http.client
import json
import os
from pathlib import Path
import pwd
import socket
import ssl
import stat
import struct
import subprocess
import sys
import tempfile
import time

TARGETS = ('veil-local-ban-pi-test', 'veil-local-ban-pi-full')


def select_target(name):
    # Only these separately approved fixtures; never accept arbitrary root paths.
    if name not in TARGETS:
        raise ValueError('Unknown fixed test target')
    global NAME, UNIT, OPT, STATE, FRAGMENT, DROPINS
    NAME = name
    UNIT = NAME + '.service'
    OPT = Path('/opt') / NAME
    STATE = Path('/var/lib') / NAME
    FRAGMENT = Path('/etc/systemd/system') / UNIT
    DROPINS = Path(str(FRAGMENT) + '.d')


select_target(TARGETS[0])
NAMESPACE = '/run/netns/ns-veil-rogue'
BINARY_SHA = 'f9cf2dc594cc0808b91247a8f0d447edcb7b9370ae8273e0f777deebe85da05a'
OBJECT_SHA = '2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f'
CLIENT = '''import json,socket,struct,sys
r=json.loads(sys.argv[2]);frame=struct.pack('>4sB3x16sQII',b'VLB1',r['op'],bytes.fromhex(r['instance']) if r['instance'] else bytes(16),r['revision'],int.from_bytes(socket.inet_aton(r['ip']),'big'),r['ttl'])
with socket.socket(socket.AF_UNIX) as s:
 s.settimeout(2.5);s.connect(sys.argv[1]);s.sendall(frame);s.shutdown(socket.SHUT_WR);data=b''
 while len(data)<=4096:
  part=s.recv(4097-len(data))
  if not part:break
  data+=part
lines=data.decode().splitlines();assert lines and len(data)<=4096
print(json.dumps(dict(ok=lines[0]=='OK',**dict(v.split('=',1) for v in lines[1:] if '=' in v))))
'''


def run(args, check=True, timeout=20):
    p = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    if check and p.returncode:
        raise RuntimeError((args, p.returncode, p.stdout, p.stderr))
    return p


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def show():
    p = run(['systemctl', 'show', UNIT, '--no-pager'])
    return dict(v.split('=', 1) for v in p.stdout.splitlines() if '=' in v)


def reset_failed():
    p = run(['systemctl', 'reset-failed', UNIT], check=False)
    if p.returncode:
        # A permanent file is not a permanent in-memory unit object. Inactive
        # units may be collected; ResetFailedUnit does not load them. Accept
        # only this specific case after checking the freshly loaded fragment.
        state = show()
        if not ('not loaded' in p.stderr and state['LoadState'] == 'loaded'
                and state['FragmentPath'] == str(FRAGMENT) and state['ActiveState'] == 'inactive'
                and state['MainPID'] == '0' and state['Result'] == 'success' and state['NRestarts'] == '0'):
            raise RuntimeError(('reset-failed', p.returncode, p.stdout, p.stderr))
        return {'result': 'inactive_object_collected_no_failure_to_reset', 'stderr': p.stderr}
    return {'result': 'reset_performed'}


def link():
    return json.loads(run(['ip', '-j', '-details', '-n', 'ns-veil-rogue', 'link', 'show', 'veth-rogue']).stdout)[0]


def attached():
    return bool(link().get('xdp', {}).get('attached'))


def wait(predicate, seconds=8):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        if predicate():
            return True
        time.sleep(.05)
    return False


def public_accounts():
    # Never read shadow/gshadow or record passwd/group contents.
    return {name: [hashlib.sha256(v).hexdigest() for v in Path('/etc', name).read_bytes().splitlines()]
            for name in ['passwd', 'group']}


def critical_services():
    rows = {}
    for unit in ['ssh.service', 'docker.service', 'systemd-timesyncd.service', 'ufw.service']:
        p = run(['systemctl', 'show', unit, '-p', 'LoadState', '-p', 'ActiveState', '-p', 'MainPID'], check=False)
        rows[unit] = p.stdout
    return rows


def exclusive_file(target, data, mode):
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    try:
        with os.fdopen(fd, 'wb') as output:
            output.write(data)
            output.flush()
            os.fchmod(output.fileno(), mode)  # New O_EXCL file only; independent of private artifact umask.
            os.fsync(output.fileno())
    except BaseException:
        # Preserve this partial file as evidence, never silently remove it.
        raise


def fresh_directory(path, mode, account=None):
    path.mkdir(mode=mode)
    os.chmod(path, mode)  # This function has just created the path exclusively.
    if account:
        os.chown(path, account.pw_uid, account.pw_gid)


def absent_preflight():
    for name in [OPT, STATE, FRAGMENT, DROPINS, Path(str(FRAGMENT) + '.disabled')]:
        assert not os.path.lexists(name), f'Preserve existing path: {name}'
    for lookup in [pwd.getpwnam, grp.getgrnam]:
        try:
            lookup(NAME)
        except KeyError:
            pass
        else:
            raise RuntimeError('Preserve existing account/group')
    state = show()
    assert state['LoadState'] == 'not-found' and not state.get('DropInPaths'), state
    assert not attached(), 'Preserve existing XDP'
    p = run(['ip', 'netns', 'exec', 'ns-veil-rogue', 'ss', '-Hlnt', '( sport = :18443 or sport = :18444 )'])
    assert not p.stdout.strip(), 'Preserve existing listener'


def validate_install():
    manifest = json.loads((OPT / 'installation.json').read_text())
    account = pwd.getpwnam(NAME)
    assert account.pw_uid == manifest['uid'] != 0
    assert account.pw_gid == manifest['gid'] == grp.getgrnam(NAME).gr_gid
    assert account.pw_dir == '/nonexistent' and account.pw_shell == '/usr/sbin/nologin'
    assert not [g.gr_name for g in grp.getgrall() if NAME in g.gr_mem and g.gr_gid != account.pw_gid]
    assert sha(FRAGMENT) == manifest['files'][str(FRAGMENT)]
    assert sha(OPT / 'veil-local-ban') == BINARY_SHA
    assert sha(OPT / 'veil_xdp_kern.o') == OBJECT_SHA
    for path, digest in manifest['files'].items():
        assert sha(path) == digest, path
    return account, manifest


def install(args, evidence, root):
    absent_preflight()
    bundle = Path(args.bundle)
    expected = json.loads((bundle.parent / 'bundle-sha256.json').read_text())
    assert all(sha(bundle / name) == digest for name, digest in expected.items())
    assert sha(args.binary) == BINARY_SHA and sha(args.object) == OBJECT_SHA
    evidence['public_accounts_before'] = public_accounts()
    evidence['critical_before'] = critical_services()
    evidence['link_before'] = link()
    fresh_directory(OPT, 0o755)
    for source, target, mode in [(Path(args.binary), OPT / 'veil-local-ban', 0o555),
                                 (Path(args.object), OPT / 'veil_xdp_kern.o', 0o444),
                                 (bundle / 'service.conf', OPT / 'service.conf', 0o600),
                                 (bundle / 'sysusers.conf', OPT / 'sysusers.conf', 0o444),
                                 (bundle / '90-recovery.conf', OPT / '90-recovery.conf', 0o444)]:
        exclusive_file(target, source.read_bytes(), mode)
    p = run(['systemd-sysusers', str(OPT / 'sysusers.conf')])
    (root / 'sysusers.log').write_text(p.stdout + p.stderr)
    account = pwd.getpwnam(NAME)
    assert account.pw_uid != 0 and account.pw_gid == grp.getgrnam(NAME).gr_gid
    assert not run(['pgrep', '-u', str(account.pw_uid)], check=False).stdout.strip()
    locked = run(['passwd', '-S', NAME]).stdout
    assert locked.split()[1] == 'L', locked
    evidence['account_status'] = locked
    fresh_directory(STATE, 0o755)
    fresh_directory(STATE / 'primary', 0o700, account)
    fresh_directory(STATE / 'recovery', 0o700, account)
    fresh_directory(DROPINS, 0o755)
    exclusive_file(DROPINS / '10-isolation.conf', (bundle / '10-isolation.conf').read_bytes(), 0o644)
    exclusive_file(FRAGMENT, (bundle / (UNIT)).read_bytes(), 0o644)
    p = run(['systemd-analyze', 'verify', str(FRAGMENT)])
    (root / 'unit-verify.log').write_text(p.stdout + p.stderr)
    files = [FRAGMENT, DROPINS / '10-isolation.conf', OPT / 'veil-local-ban', OPT / 'veil_xdp_kern.o',
             OPT / 'service.conf', OPT / 'sysusers.conf', OPT / '90-recovery.conf']
    manifest = {'uid': account.pw_uid, 'gid': account.pw_gid, 'files': {str(p): sha(p) for p in files},
                'bundle_sha256': expected, 'artifact_dir': str(root),
                'public_accounts_before': evidence['public_accounts_before'], 'critical_before': evidence['critical_before']}
    exclusive_file(OPT / 'installation.json', json.dumps(manifest, indent=2).encode(), 0o600)
    validate_install()
    run(['systemctl', 'daemon-reload'])
    state = show()
    assert state['LoadState'] == 'loaded' and state['ActiveState'] == 'inactive'
    assert state['FragmentPath'] == str(FRAGMENT) and state['Transient'] == 'no'
    assert state['UnitFileState'] == 'disabled', state['UnitFileState']
    evidence['installed_state'] = state
    after = public_accounts()
    assert all(set(rows).issubset(after[name]) for name, rows in evidence['public_accounts_before'].items())
    evidence['critical_after'] = critical_services()
    assert evidence['critical_before'] == evidence['critical_after'], 'Investigate critical service changes; never restart another service'
    evidence['uid'], evidence['gid'] = account.pw_uid, account.pw_gid


def accept(args, evidence, root):
    account, manifest = validate_install()
    primary, recovery = STATE / 'primary', STATE / 'recovery'
    proof = primary / 'socket-owner'
    previous = None
    resume_tail = False
    if args.resume_result:
        previous = json.loads(Path(args.resume_result).read_text())
        assert previous['phase'] == 'accept' and not previous['complete'] and previous['cleanup_ok']
        last = previous['checks'][-1]
        if (last == {'name': 'inspection_not_repair', 'passed': True}
                and 'reset-failed' in previous['error'] and 'not loaded' in previous['error']):
            assert not list(recovery.iterdir()) and not os.path.lexists(DROPINS / '90-recovery.conf')
        else:
            assert last == {'name': 'dedicated_uid_and_zero_runtime_caps', 'passed': True}
            assert 'UnboundLocalError' in previous['error'] and 'proof' in previous['error']
            assert previous.get('continued_from')
            assert sha(DROPINS / '90-recovery.conf') == sha(OPT / '90-recovery.conf')
            assert sorted(p.name for p in recovery.iterdir()) == ['socket-owner']
            record = (recovery / 'socket-owner').lstat()
            assert (stat.S_ISREG(record.st_mode) and stat.S_IMODE(record.st_mode) == 0o600
                    and record.st_uid == account.pw_uid and record.st_gid == account.pw_gid
                    and record.st_size == 72 and record.st_nlink == 1)
            inspected = run(['setpriv', f'--reuid={account.pw_uid}', f'--regid={account.pw_gid}', '--clear-groups',
                             str(OPT / 'veil-local-ban'), 'inspect-runtime', '--control-dir', str(recovery)])
            assert 'owner_record=valid\n' in inspected.stdout and 'pending_record=missing\n' in inspected.stdout
            resume_tail = True
        evidence['continued_from'] = str(Path(args.resume_result).resolve())
        evidence['resume_stage'] = 'recovery_verification' if resume_tail else 'corruption_and_recovery'
    else:
        assert not list(primary.iterdir()) and not list(recovery.iterdir()), 'Preserve prior fixture; do not auto-clean or repeat acceptance'
    assert show()['ActiveState'] == 'inactive' and not attached()
    evidence['critical_before'] = critical_services()
    server = None
    log = None
    def save():
        (root / 'result.json').write_text(json.dumps(evidence, indent=2))
    def check(name, condition, **details):
        evidence['checks'].append(dict(name=name, passed=bool(condition), **details)); save()
        print(('PASS ' if condition else 'FAIL ') + name, flush=True)
        assert condition, (name, details)
    def client(directory, op=0, ttl=0, instance=None, revision=0, ip='10.99.0.1'):
        request = json.dumps(dict(op=op, ttl=ttl, instance=instance, revision=revision, ip=ip))
        p = run(['setpriv', f'--reuid={account.pw_uid}', f'--regid={account.pw_gid}', '--clear-groups',
                 '/usr/bin/python3', '-c', CLIENT, str(directory / 'control.sock'), request], timeout=5)
        return json.loads(p.stdout)
    def mutate(directory, op=1, ttl=0, ip='10.99.0.1'):
        state = client(directory); time.sleep(.21)
        return client(directory, op, ttl, state['instance'], int(state['revision']), ip)
    def ownership(directory):
        return {p.name: {'uid': p.lstat().st_uid, 'mode': p.lstat().st_mode, 'inode': p.lstat().st_ino,
                         'sha256': sha(p) if p.is_file() and not p.is_symlink() else None} for p in directory.iterdir()}
    if previous:
        before = previous['original_preservation_after_stop']
        assert ownership(primary) == before, 'Do not adopt a changed or unknown fixture'
    def start():
        run(['systemctl', 'start', UNIT]); state = show()
        check('permanent_unit_active', state['ActiveState'] == 'active' and state['FragmentPath'] == str(FRAGMENT)
              and state['Transient'] == 'no' and state['UnitFileState'] == 'disabled')
        return client(primary)
    def stop():
        run(['systemctl', 'stop', UNIT]); state = show()
        check('stop_detaches_and_keeps_fragment', state['ActiveState'] == 'inactive' and not attached()
              and state['LoadState'] == 'loaded' and state['FragmentPath'] == str(FRAGMENT))
    def caps():
        pid = show()['MainPID']; rows = []
        for task in Path(f'/proc/{pid}/task').iterdir():
            fields = dict(v.split(':', 1) for v in (task / 'status').read_text().splitlines() if ':' in v)
            rows.append({k: fields[k].strip() for k in ['Uid', 'Groups', 'CapEff', 'CapPrm', 'CapInh', 'CapAmb', 'CapBnd', 'NoNewPrivs']})
        check('dedicated_uid_and_zero_runtime_caps', bool(rows) and all(int(r['Uid'].split()[0]) == account.pw_uid
              and all(int(g) == account.pw_gid for g in r['Groups'].split())
              and all(int(r[k], 16) == 0 for k in ['CapEff', 'CapPrm', 'CapInh', 'CapAmb']) and r['NoNewPrivs'] == '1' for r in rows))
        evidence.setdefault('capability_snapshots', []).append(rows)
    cert, key = root / 'cert.pem', root / 'test-key.pem'
    run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=isolated-permanent-unit',
         '-addext', 'subjectAltName=IP:10.99.0.66', '-keyout', str(key), '-out', str(cert)])
    context = ssl.create_default_context(cafile=str(cert)); token = os.urandom(8).hex()
    def https(port=18443):
        try:
            c = http.client.HTTPSConnection('10.99.0.66', port, context=context, timeout=1, source_address=('10.99.0.1', 0))
            try:
                c.request('GET', '/'); r = c.getresponse(); return r.status == 200 and r.read() == token.encode()
            finally:
                c.close()
        except (OSError, http.client.HTTPException):
            return False
    try:
        log = (root / 'https.log').open('wb')
        server = subprocess.Popen(['ip', 'netns', 'exec', 'ns-veil-rogue', '/usr/bin/python3', args.http_helper,
                                   'server', str(cert), str(key), token], stdout=log, stderr=subprocess.STDOUT)
        check('https_baseline', wait(https, 5) and https(18444))
        if not previous:
            first = start(); check('empty_256_contract', first['capacity'] == '256' and first['occupied'] == '0'); caps()
            p = run([str(OPT / 'veil-local-ban'), 'status', '--control-dir', str(primary)], check=False)
            check('nonservice_uid_cli_rejected', p.returncode != 0)
            p = run(['setpriv', f'--reuid={account.pw_uid}', f'--regid={account.pw_gid}', '--clear-groups',
                     str(OPT / 'veil-local-ban'), 'status', '--control-dir', str(primary)])
            check('real_cli_same_uid', p.stdout.startswith('OK\n'))
            check('ban', mutate(primary, ttl=3)['ok']); check('ban_drop_and_scope', not https() and https(18444))
            check('kernel_ttl_pass', wait(https, 5)); check('bounded_cleanup', wait(lambda: client(primary)['occupied'] == '0', 4))
            check('explicit_ban', mutate(primary, ttl=30)['ok']); check('explicit_unban', mutate(primary, 2)['ok'] and https())
            check('capacity_first_actual_source', mutate(primary, ttl=180)['ok'])
            for index in range(255):
                response = mutate(primary, ttl=180, ip=f'198.18.0.{index}')
                assert response['ok'], (index, response)
                if index % 64 == 63: print(f'CAPACITY {index+2}/256', flush=True)
            check('full_256', client(primary)['occupied'] == '256')
            response = mutate(primary, ttl=180, ip='198.19.0.1')
            check('257_rejected_no_eviction', response.get('code') == 'CAPACITY' and not https() and https(18444))
            old = client(primary)['instance']; run(['systemctl', 'restart', UNIT])
            check('restart_clears_ban', client(primary)['instance'] != old and client(primary)['occupied'] == '0' and https())
            check('ban_before_kill', mutate(primary, ttl=30)['ok'])
            old = client(primary)['instance']; old_pid = show()['MainPID']
            run(['systemctl', 'kill', '--signal=SIGKILL', '--kill-whom=main', UNIT])
            check('sigkill_automatic_restart', wait(lambda: show()['ActiveState'] == 'active' and show()['MainPID'] not in ['0', old_pid]))
            check('sigkill_empty_new_instance', client(primary)['instance'] != old and client(primary)['occupied'] == '0' and https()); caps()
            stop(); check('stop_preserves_owner_record', sorted(p.name for p in primary.iterdir()) == ['socket-owner'])
            start(); check('same_permanent_unit_can_start_again', client(primary)['occupied'] == '0' and https()); stop()
            proof = primary / 'socket-owner'
            exclusive_file(root / 'original-owner-record.bin', proof.read_bytes(), 0o600)
            proof.write_bytes(b'owned test fixture: deliberately corrupt and preserve')
            exclusive_file(primary / 'keep.txt', b'unknown-file fixture: preserve exactly', 0o600)
            before = ownership(primary)
            p = run(['setpriv', f'--reuid={account.pw_uid}', f'--regid={account.pw_gid}', '--clear-groups',
                     str(OPT / 'veil-local-ban'), 'inspect-runtime', '--control-dir', str(primary)])
            check('inspection_not_repair', p.stdout.startswith('INSPECTED\n') and ownership(primary) == before)
        else:
            check('resumed_original_fixture_unchanged', ownership(primary) == before)
        if not resume_tail:
            evidence.setdefault('reset_operations', []).append(reset_failed())
            run(['systemctl', 'start', UNIT], check=False)
            state = show(); check('corruption_refused', state['ActiveState'] == 'failed' and state['ExecMainStatus'] == '1' and state['Result'] == 'exit-code')
            time.sleep(2.2)
            check('no_corruption_restart_loop', show()['NRestarts'] == '0' and not attached() and ownership(primary) == before)
            assert not list(recovery.iterdir())
            exclusive_file(DROPINS / '90-recovery.conf', (OPT / '90-recovery.conf').read_bytes(), 0o644)
            run(['systemd-analyze', 'verify', str(FRAGMENT)]); run(['systemctl', 'daemon-reload']); evidence.setdefault('reset_operations', []).append(reset_failed()); run(['systemctl', 'start', UNIT])
        else:
            run(['systemctl', 'start', UNIT])
        state = show(); check('same_unit_recovers', state['ActiveState'] == 'active' and state['Transient'] == 'no' and state['FragmentPath'] == str(FRAGMENT)
              and str(DROPINS / '90-recovery.conf') in state['DropInPaths'])
        check('recovery_empty_https', client(recovery)['occupied'] == '0' and https()); caps()
        probe = 'import errno,os,sys\ntry:\n fd=os.open(sys.argv[1],os.O_WRONLY);os.close(fd)\nexcept OSError as e:\n sys.exit(0 if e.errno==errno.EROFS else 2)\nsys.exit(1)'
        p = run(['nsenter', '--target', state['MainPID'], '--mount', '--root', '--wd=/', '--', 'setpriv',
                 f'--reuid={account.pw_uid}', f'--regid={account.pw_gid}', '--clear-groups', '/usr/bin/python3', '-c', probe, str(proof)], check=False)
        check('original_readonly_and_preserved', p.returncode == 0 and ownership(primary) == before)
        old_recovery_instance = client(recovery)['instance']
        check('recovery_ban_before_restart', mutate(recovery, ttl=30)['ok'] and not https())
        run(['systemctl', 'restart', UNIT])
        check('recovery_restart_stays_empty', client(recovery)['instance'] != old_recovery_instance and client(recovery)['occupied'] == '0' and https() and ownership(primary) == before)
        stop(); check('not_enabled', show()['UnitFileState'] == 'disabled')
        after = public_accounts()
        check('other_public_accounts_preserved', all(set(v).issubset(after[k]) for k, v in manifest['public_accounts_before'].items()))
        evidence['critical_after'] = critical_services()
        check('critical_services_not_restarted', evidence['critical_before'] == evidence['critical_after'])
    finally:
        # These are the one fixture unit and child process created by this task.
        if sha(FRAGMENT) == manifest['files'][str(FRAGMENT)]:
            run(['systemctl', 'stop', UNIT], check=False)
        if server and server.poll() is None:
            server.terminate(); server.wait(timeout=5)
        if log:
            log.close()
        evidence['link_after'] = link()
        evidence['cleanup_ok'] = not attached() and (server is None or server.poll() is not None) and show()['ActiveState'] == 'inactive'
        evidence['unit_after'] = show()
        evidence['recovery_preservation_after_stop'] = ownership(recovery)
        evidence['original_preservation_after_stop'] = ownership(primary) if 'before' in locals() else None
        if 'before' in locals():
            assert evidence['original_preservation_after_stop'] == before
        save()


def main():
    p = argparse.ArgumentParser(); p.add_argument('phase', choices=['preflight', 'install', 'accept'])
    p.add_argument('--target', choices=TARGETS, default=TARGETS[0])
    p.add_argument('--apply', action='store_true'); p.add_argument('--bundle'); p.add_argument('--binary'); p.add_argument('--object')
    p.add_argument('--artifacts-parent'); p.add_argument('--http-helper'); p.add_argument('--resume-result'); args = p.parse_args()
    if args.target == 'veil-local-ban-pi-full' and args.resume_result:
        p.error('The fresh full-run target must never resume a prior acceptance')
    select_target(args.target)
    assert os.geteuid() == 0
    if args.phase == 'preflight':
        absent_preflight(); print('PREFLIGHT ONLY: fixed targets absent; isolated scope idle'); return
    assert args.apply, 'Explicit authorization is required separately; --apply only prevents accidental execution'
    assert args.artifacts_parent
    os.umask(0o077)
    root = Path(tempfile.mkdtemp(prefix='permanent-'+args.phase+'-', dir=args.artifacts_parent))
    evidence = {'artifact_dir': str(root), 'phase': args.phase, 'target': NAME, 'checks': [], 'complete': False,
                'driver_sha256': sha(__file__), 'expected_binary_sha256': BINARY_SHA, 'expected_object_sha256': OBJECT_SHA}
    try:
        if args.phase == 'install':
            assert args.bundle and args.binary and args.object
            install(args, evidence, root)
        else:
            assert args.http_helper
            accept(args, evidence, root)
        evidence['complete'] = True
    except BaseException as error:
        evidence['error'] = repr(error); raise
    finally:
        evidence['full_single_run_passed'] = bool(args.phase == 'accept' and evidence['complete'] and not args.resume_result)
        (root / 'result.json').write_text(json.dumps(evidence, indent=2))
        print('ARTIFACTS='+str(root), flush=True)


if __name__ == '__main__':
    main()
