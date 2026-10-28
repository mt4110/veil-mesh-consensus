#!/usr/bin/env python3
"""Fixed fresh-binary acceptance; only /run and a transient unit are created.

The new binary is read-only bound over an existing executable path inside the
test unit alone. Never install/enable a unit, replace host files or reboot.
"""
import argparse
import configparser
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import pwd
import ssl
import subprocess
import tempfile
import time
import uuid

FRESH = Path('/home/maakie/vnr-a4dsem9l/target/debug/veil-local-ban')
FRESH_SHA = '00dff5be88570c7506d05995cae79fac03614c18a24a6c16cf2431416881cc19'
HOST_BINARY = Path('/opt/veil-local-ban-pi-full/veil-local-ban')
HOST_SHA = 'f9cf2dc594cc0808b91247a8f0d447edcb7b9370ae8273e0f777deebe85da05a'
OBJECT = Path('/opt/veil-local-ban-pi-full/veil_xdp_kern.o')
OBJECT_SHA = '2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f'
PREVIOUS = Path('/home/maakie/vlb-boot-f4d85_9u')


def require(ok, message):
    if not ok:
        raise RuntimeError(message)


def run(args, check=True, timeout=20):
    p = subprocess.run([str(v) for v in args], capture_output=True, text=True, timeout=timeout)
    require(not check or p.returncode == 0, repr((args, p.returncode, p.stdout, p.stderr)))
    return p


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def show(unit):
    return dict(v.split('=', 1) for v in run(['systemctl', 'show', unit]).stdout.splitlines() if '=' in v)


def wait(predicate, seconds=8):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if predicate():
            return True
        time.sleep(.05)
    return False


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec); spec.loader.exec_module(result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__); parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    require(args.apply and os.geteuid() == 0, 'Root and --apply required; human authorization is separate')
    require(sha(FRESH) == FRESH_SHA and sha(HOST_BINARY) == HOST_SHA and sha(OBJECT) == OBJECT_SHA, 'Unexpected executable/object')
    account = pwd.getpwnam('veil-local-ban-pi-full')
    require(account.pw_uid == account.pw_gid == 979, 'Unexpected dedicated account')
    require(not run(['pgrep', '-u', '979'], check=False).stdout.strip(), 'Preserve existing service-UID process')
    inspect = run(['/usr/bin/python3', '-I', '-B', '/opt/veil-local-ban-pi-boot/namespace.py', 'inspect'])
    require(json.loads(inspect.stdout) == dict(result='verified_existing', changed=False), 'Preserve unknown namespace')
    require(not run(['ip', 'netns', 'exec', 'ns-veil-rogue', 'ss', '-Hlnt', '( sport = :18443 or sport = :18444 )']).stdout.strip(), 'Preserve existing listener')
    os.umask(0o077)
    root = Path(tempfile.mkdtemp(prefix='veil-rebuilt-evidence-', dir='/run'))
    runtime = Path(tempfile.mkdtemp(prefix='veil-rebuilt-runtime-', dir='/run')); runtime.chmod(0o755)
    control = runtime / 'control'; control.mkdir(mode=0o700); os.chown(control, 979, 979)
    unit = 'veil-rebuilt-' + uuid.uuid4().hex[:12] + '.service'
    require(show(unit)['LoadState'] == 'not-found', 'Preserve existing unit')
    support = module('support', PREVIOUS / 'boot-driver-v2.py')
    wire = module('wire', PREVIOUS / 'permanent-driver.py').CLIENT
    evidence = dict(complete=False, checks=[], unit=unit, artifact_dir=str(root), runtime_dir=str(runtime),
                    driver_sha256=sha(__file__), fresh_sha256=FRESH_SHA, host_binary_sha256=HOST_SHA, object_sha256=OBJECT_SHA)
    def save():
        (root / 'result.json').write_text(json.dumps(evidence, indent=2))
    def check(name, ok):
        evidence['checks'].append(dict(name=name, passed=bool(ok))); save()
        print(('PASS ' if ok else 'FAIL ') + name, flush=True); require(ok, name)
    def snapshot():
        data = support.audit(PREVIOUS)
        extra = {}
        for base in [Path('/var/lib/veil-local-ban-pi-boot'), Path('/opt/veil-local-ban-pi-boot')]:
            for p in [base, *base.rglob('*')]:
                s = p.lstat()
                extra[str(p)] = dict(inode=s.st_ino, mode=s.st_mode, uid=s.st_uid, gid=s.st_gid,
                                     sha256=sha(p) if p.is_file() and not p.is_symlink() else None)
        data['extra_boot_fixture'] = extra
        data['test_unit_states'] = {u: {k: show(u).get(k) for k in ['ActiveState', 'UnitFileState', 'MainPID']}
            for u in ['veil-local-ban-pi-boot.service', 'veil-local-ban-pi-namespace.service',
                      'veil-local-ban-pi-full.service', 'veil-local-ban-pi-test.service']}
        return data
    before = snapshot(); (root / 'before.json').write_text(json.dumps(before, indent=2))
    require(all(s['ActiveState'] == 'inactive' and s['UnitFileState'] == 'disabled' and s['MainPID'] == '0'
                for s in before['test_unit_states'].values()), 'Preserve active prior test units')
    def client(op=0, ttl=0, instance=None, revision=0, ip='10.99.0.1'):
        payload = json.dumps(dict(op=op, ttl=ttl, instance=instance, revision=revision, ip=ip))
        return json.loads(run(['setpriv', '--reuid=979', '--regid=979', '--clear-groups', '/usr/bin/python3', '-I', '-c',
                               wire, control / 'control.sock', payload], timeout=5).stdout)
    def mutate(op=1, ttl=0, ip='10.99.0.1'):
        s = client(); time.sleep(.21)
        return client(op, ttl, s['instance'], int(s['revision']), ip)
    def verify_executable_and_privileges(label):
        s = show(unit); pid = s['MainPID']
        digest = sha(f'/proc/{pid}/exe'); evidence.setdefault('executable_snapshots', []).append(dict(label=label, pid=pid, sha256=digest))
        check(label + '_actual_executable_is_rebuilt_binary', digest == FRESH_SHA and sha(HOST_BINARY) == HOST_SHA and show(unit)['MainPID'] == pid)
        check(label + '_in_isolated_namespace', os.stat(f'/proc/{pid}/ns/net').st_ino == os.stat('/run/netns/ns-veil-rogue').st_ino)
        rows = []
        for task in Path(f'/proc/{pid}/task').iterdir():
            f = dict(v.split(':', 1) for v in (task / 'status').read_text().splitlines() if ':' in v)
            rows.append({k: f[k].strip() for k in ['Uid', 'Groups', 'CapEff', 'CapPrm', 'CapInh', 'CapAmb', 'NoNewPrivs']})
        evidence.setdefault('privilege_snapshots', []).append(rows)
        check(label + '_all_threads_uid979_caps_zero', bool(rows) and all(all(int(v) == 979 for v in r['Uid'].split())
              and r['Groups'] == '979' and r['NoNewPrivs'] == '1'
              and all(int(r[k], 16) == 0 for k in ['CapEff', 'CapPrm', 'CapInh', 'CapAmb']) for r in rows))
    profile = configparser.ConfigParser(interpolation=None); profile.optionxform = str
    profile.read_string(run(['cat', '/etc/systemd/system/veil-local-ban-pi-full.service']).stdout)
    properties = {k: v for k, v in profile['Service'].items() if k not in
                  ['Type', 'User', 'Group', 'ExecStart', 'EnvironmentFile', 'ReadOnlyPaths', 'ReadWritePaths']}
    properties.update(User='veil-local-ban-pi-full', Group='veil-local-ban-pi-full',
        NetworkNamespacePath='/run/netns/ns-veil-rogue', BindReadOnlyPaths=f'{FRESH}:{HOST_BINARY}',
        ReadOnlyPaths=f'{runtime} /var/lib/veil-local-ban-pi-full /var/lib/veil-local-ban-pi-test /var/lib/veil-local-ban-pi-boot',
        ReadWritePaths=str(control))
    properties.update({k: v for k, v in profile['Unit'].items() if k.startswith('StartLimit')})
    command = ['systemd-run', '--unit=' + unit, '--service-type=notify', '--quiet']
    for k, v in properties.items(): command += ['--property=' + k + '=' + v]
    command += [str(HOST_BINARY), 'serve', '--systemd-notify', '--iface', 'veth-rogue', '--ebpf-object', str(OBJECT),
                '--control-dir', str(control), '--destination', '10.99.0.66', '--port', '18443', '--protected-ip', '198.51.100.7']
    (root / 'command.json').write_text(json.dumps(command, indent=2))
    server = log = None; attempted = False
    cert, key = root / 'cert.pem', root / 'test-key.pem'
    try:
        run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=isolated-rebuilt',
             '-addext', 'subjectAltName=IP:10.99.0.66', '-keyout', key, '-out', cert])
        token = os.urandom(8).hex(); context = ssl.create_default_context(cafile=str(cert))
        def https(port=18443):
            try:
                c = http.client.HTTPSConnection('10.99.0.66', port, context=context, timeout=1, source_address=('10.99.0.1', 0))
                try:
                    c.request('GET', '/'); r = c.getresponse(); return r.status == 200 and r.read() == token.encode()
                finally: c.close()
            except (OSError, http.client.HTTPException): return False
        log = (root / 'https.log').open('xb')
        server = subprocess.Popen(['ip', 'netns', 'exec', 'ns-veil-rogue', '/usr/bin/python3', '-I', '-B',
                                   PREVIOUS / 'http-helper.py', 'server', cert, key, token], stdout=log, stderr=subprocess.STDOUT)
        check('normal_https_before_daemon', wait(https, 5) and https(18444))
        attempted = True; run(command)
        check('ready_xdp_and_empty_256', show(unit)['ActiveState'] == 'active' and support.link().get('xdp', {}).get('attached')
              and client()['capacity'] == '256' and client()['occupied'] == '0')
        verify_executable_and_privileges('initial')
        check('normal_https_after_daemon', https() and https(18444))
        check('zero_ttl_refused', mutate(ttl=0).get('code') == 'INVALID_TTL')
        check('short_ban_scope', mutate(ttl=3)['ok'] and not https() and https(18444) and int(client()['kernel_drop']) > 0)
        run(['systemctl', 'kill', '--signal=SIGSTOP', '--kill-whom=main', unit])
        time.sleep(3.1); check('kernel_expiry_while_daemon_paused', https())
        run(['systemctl', 'kill', '--signal=SIGCONT', '--kill-whom=main', unit])
        check('bounded_cleanup_after_resume', wait(lambda: client().get('occupied') == '0', 4))
        s = client(); check('cleanup_budget_16', int(s['max_examined']) <= 16 and int(s['max_attempted']) <= 16)
        check('ban_before_unban', mutate(ttl=30)['ok'] and not https())
        check('individual_unban', mutate(2)['ok'] and https())
        check('first_actual_source_in_capacity_test', mutate(ttl=180)['ok'])
        for i in range(255):
            require(mutate(ttl=180, ip=f'198.18.0.{i+1}')['ok'], 'Capacity input ' + str(i))
            if i % 64 == 63: print(f'CAPACITY {i+2}/256', flush=True)
        s = client(); evidence['full_status'] = s
        check('capacity_256_active', s['occupied'] == s['active'] == '256')
        check('257_rejected_no_eviction', mutate(ttl=180, ip='198.19.0.1').get('code') == 'CAPACITY' and not https() and https(18444))
        run(['systemctl', 'restart', unit]); new = client()
        check('restart_clears_full_table', new['instance'] != s['instance'] and new['occupied'] == '0' and https() and https(18444))
        check('old_instance_refused', client(1, 30, s['instance'], int(s['revision'])).get('code') == 'INSTANCE_CONFLICT' and client()['occupied'] == '0')
        verify_executable_and_privileges('normal_restart')
        check('ban_before_sigkill', mutate(ttl=30)['ok'] and not https())
        old_pid = show(unit)['MainPID']; old_instance = client()['instance']
        run(['systemctl', 'kill', '--signal=SIGKILL', '--kill-whom=main', unit])
        check('automatic_restart_ready', wait(lambda: show(unit)['ActiveState'] == 'active' and show(unit)['MainPID'] not in ['0', old_pid]))
        check('sigkill_restart_clears_ban', client()['instance'] != old_instance and client()['occupied'] == '0' and https())
        verify_executable_and_privileges('sigkill_restart')
        evidence['complete'] = True
    except BaseException as error:
        evidence['error'] = repr(error); raise
    finally:
        if attempted:
            state = show(unit)
            if state['LoadState'] == 'loaded':
                require(state.get('Transient') == 'yes' and state.get('FragmentPath') == '/run/systemd/transient/' + unit, 'Preserve unknown unit')
                run(['systemctl', 'kill', '--signal=SIGCONT', '--kill-whom=main', unit], check=False)
                run(['systemctl', 'stop', unit])
                (root / 'unit-journal.log').write_text(run(['journalctl', '-u', unit, '--no-pager', '-o', 'short-iso']).stdout)
        if server is not None and server.poll() is None: server.terminate(); server.wait(timeout=5)
        if log is not None: log.close()
        evidence['unit_after'] = show(unit); evidence['link_after'] = support.link()
        evidence['runtime_entries'] = sorted(p.name for p in control.iterdir())
        after = snapshot(); (root / 'after.json').write_text(json.dumps(after, indent=2))
        same = ['existing_fixture', 'new_fixture', 'extra_boot_fixture', 'service_file_hashes', 'services',
                'mainnet', 'public_accounts', 'boot_id', 'test_unit_states']
        evidence['preservation'] = {k: before[k] == after[k] for k in same}
        fields = ['ifindex', 'ifname', 'address', 'mtu', 'flags', 'xdp']
        normalize = lambda data: [{k: row.get(k) for k in fields} for row in json.loads(data['host_links']['stdout'])]
        evidence['preservation']['host_links_unchanged'] = normalize(before) == normalize(after)
        evidence['preservation']['fresh_binary_unchanged'] = sha(FRESH) == FRESH_SHA
        evidence['preservation']['host_binary_unchanged'] = sha(HOST_BINARY) == HOST_SHA
        evidence['cleanup_ok'] = (not support.link().get('xdp', {}).get('attached')
            and evidence['unit_after']['ActiveState'] == 'inactive' and evidence['unit_after']['MainPID'] == '0'
            and not run(['pgrep', '-u', '979'], check=False).stdout.strip()
            and not run(['ip', 'netns', 'exec', 'ns-veil-rogue', 'ss', '-Hlnt', '( sport = :18443 or sport = :18444 )']).stdout.strip())
        if not evidence['cleanup_ok'] or not all(evidence['preservation'].values()):
            evidence['functional_checks_passed'] = evidence['complete']; evidence['complete'] = False
        save(); print('ARTIFACTS=' + str(root), flush=True)
        require(evidence['cleanup_ok'] and all(evidence['preservation'].values()), 'Cleanup/preservation failed; inspect retained evidence')


if __name__ == '__main__':
    main()
