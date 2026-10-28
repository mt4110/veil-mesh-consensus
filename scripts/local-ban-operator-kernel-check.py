#!/usr/bin/env python3
"""Fixed pi-node XDP acceptance of list/preview and explicitly applied commands.

Only new /run directories and one transient unit are created. Existing host
files, accounts, units, firewall, interfaces, routes and namespace are preserved.
No shell evaluation, mutation retry, deployment or host reboot.
"""
import argparse
import configparser
import http.client
import importlib.util
import json
import os
from pathlib import Path
import pwd
import shlex
import ssl
import subprocess
import tempfile
import time
import uuid


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


base = load('rebuilt_helpers', Path(__file__).with_name('local-ban-rebuilt-check.py'))
run, sha, show, wait, require = base.run, base.sha, base.show, base.wait, base.require
FRESH = Path('/home/maakie/vnr-s2dyr97k/target/debug/veil-local-ban')
FRESH_SHA = 'e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890'
HOST_BINARY, HOST_SHA, OBJECT, OBJECT_SHA, PREVIOUS = (
    base.HOST_BINARY, base.HOST_SHA, base.OBJECT, base.OBJECT_SHA, base.PREVIOUS)
HELPERS = {
    PREVIOUS / 'boot-driver-v2.py': 'aea73d8cf926f5c424d6663bf09d0f2c7a1bee66e9953fe699ae7a9944f814fa',
    PREVIOUS / 'audit-host.py': '4472ad6983e3ae52170a667bd4ca4d9c05d8bf21ebdc198f81c73ba919e6dc1a',
    PREVIOUS / 'http-helper.py': '673a9d2024d210040e6243614b71ac1381485278164d503ff660b5420bab2bfc',
    Path('/opt/veil-local-ban-pi-boot/namespace.py'): '62abf1394a4c4046159612e3a5613ca105b0bb6c14881f1f91bf6b843b431713',
}
PRIOR_UNITS = ['veil-local-ban-pi-test.service', 'veil-local-ban-pi-full.service',
               'veil-local-ban-pi-boot.service', 'veil-local-ban-pi-namespace.service']


def fields(text):
    result = {}
    for line in text.splitlines()[1:]:
        if '=' in line and not line.startswith('entry='):
            key, value = line.split('=', 1)
            require(key not in result, 'Duplicate field: ' + key)
            result[key] = value
    return result


def rows(text):
    result = []
    for line in text.splitlines():
        if line.startswith('entry='):
            entry = dict(part.split('=', 1) for part in line.split(','))
            result.append(entry)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    require(args.apply and os.geteuid() == 0, 'Root and --apply required; human authorization is separate')
    require(sha(FRESH) == FRESH_SHA and sha(HOST_BINARY) == HOST_SHA and sha(OBJECT) == OBJECT_SHA,
            'Unexpected executable/object')
    require(all(sha(p) == digest for p, digest in HELPERS.items()), 'Changed prerequisite helper')
    account = pwd.getpwnam('veil-local-ban-pi-full')
    require(account.pw_uid == account.pw_gid == 979, 'Unexpected dedicated account')
    require(not run(['pgrep', '-u', '979'], check=False).stdout.strip(), 'Preserve existing service-UID process')
    require(json.loads(run(['/usr/bin/python3', '-I', '-B', '/opt/veil-local-ban-pi-boot/namespace.py', 'inspect']).stdout)
            == dict(result='verified_existing', changed=False), 'Preserve unknown namespace')
    require(not run(['ip', 'netns', 'pids', 'ns-veil-rogue']).stdout.strip(), 'Preserve existing namespace process')
    os.umask(0o077)
    root = Path(tempfile.mkdtemp(prefix='veil-operator-evidence-', dir='/run'))
    runtime = Path(tempfile.mkdtemp(prefix='veil-operator-runtime-', dir='/run'))
    runtime.chmod(0o755)
    control = runtime / 'control'
    control.mkdir(mode=0o700)
    os.chown(control, 979, 979)
    unit = 'veil-operator-' + uuid.uuid4().hex[:12] + '.service'
    require(show(unit)['LoadState'] == 'not-found', 'Preserve unknown unit')
    support = load('boot_audit', PREVIOUS / 'boot-driver-v2.py')
    evidence = dict(complete=False, checks=[], cli=[], unit=unit, artifact_dir=str(root),
                    runtime_dir=str(runtime), driver_sha256=sha(__file__), helper_sha256=sha(base.__file__),
                    binary_sha256=FRESH_SHA, host_binary_sha256=HOST_SHA, object_sha256=OBJECT_SHA)

    def save():
        (root / 'result.json').write_text(json.dumps(evidence, indent=2))

    def check(name, ok):
        evidence['checks'].append(dict(name=name, passed=bool(ok)))
        save()
        print(('PASS ' if ok else 'FAIL ') + name, flush=True)
        require(ok, name)

    def snapshot():
        data = support.audit(PREVIOUS)
        extra = {}
        for directory in ['/var/lib/veil-local-ban-pi-boot', '/opt/veil-local-ban-pi-boot']:
            p = Path(directory)
            for item in [p, *p.rglob('*')]:
                s = item.lstat()
                extra[str(item)] = dict(inode=s.st_ino, mode=s.st_mode, uid=s.st_uid, gid=s.st_gid,
                                       sha256=sha(item) if item.is_file() and not item.is_symlink() else None)
        data['extra_boot_fixture'] = extra
        data['prior_units'] = {u: {k: show(u).get(k) for k in ['ActiveState', 'UnitFileState', 'MainPID']}
                               for u in PRIOR_UNITS}
        data['firewall'] = {name: run([name, '-S']).stdout for name in ['iptables', 'ip6tables']}
        data['ufw'] = run(['ufw', 'status', 'verbose']).stdout
        data['host_ipv6_routes'] = json.loads(run(['ip', '-j', '-6', 'route', 'show']).stdout)
        data['host_addresses'] = json.loads(run(['ip', '-j', 'address', 'show']).stdout)
        return data

    before = snapshot()
    (root / 'before.json').write_text(json.dumps(before, indent=2))
    require(all(s['ActiveState'] == 'inactive' and s['UnitFileState'] == 'disabled' and s['MainPID'] == '0'
                for s in before['prior_units'].values()), 'Preserve active prior unit')
    require(not support.link().get('xdp', {}).get('attached'), 'Preserve existing XDP')

    def cli(arguments):
        pid = show(unit)['MainPID']
        require(pid != '0', 'No live test daemon')
        command = ['nsenter', '--target', pid, '--mount', '--', 'setpriv', '--reuid=979', '--regid=979',
                   '--groups=979', '--no-new-privs', *map(str, arguments)]
        result = run(command, check=False, timeout=5)
        evidence['cli'].append(dict(argv=list(map(str, arguments)), exit_code=result.returncode,
                                    stdout=result.stdout, stderr=result.stderr))
        return result

    def read(operation, ip=None, ttl=None):
        command = [str(HOST_BINARY), operation, '--control-dir', str(control)]
        if ip is not None:
            command += ['--ip', ip]
        if ttl is not None:
            command += ['--ttl-secs', str(ttl)]
        end = time.monotonic() + 2
        while True:
            result = cli(command)
            busy = fields(result.stdout).get('code') == 'BUSY'
            partial = fields(result.stdout).get('policy_busy') == 'true'
            if not (busy or partial) or time.monotonic() >= end:
                return result
            # Only a bounded read is repeated; mutation commands never retry.
            time.sleep(.01)

    def status():
        result = read('status')
        require(result.returncode == 0 and 'revision' in fields(result.stdout), 'Status unavailable')
        return fields(result.stdout)

    def listing():
        result = read('list')
        require(result.returncode == 0, 'List refused: ' + result.stdout)
        return fields(result.stdout), rows(result.stdout), result.stdout

    def preview(operation, ip='10.99.0.1', ttl=30):
        result = read('preview-' + operation, ip, ttl if operation == 'ban' else None)
        require(result.returncode == 0, 'Preview refused: ' + result.stdout)
        f = fields(result.stdout)
        command = shlex.split(f['execute_manually'])
        expected = [str(HOST_BINARY), operation, '--control-dir', str(control), '--ip', ip]
        if operation == 'ban':
            expected += ['--ttl-secs', str(ttl)]
        expected += ['--instance', f['instance'], '--expected-revision', f['revision']]
        require(command == expected and f['ip'] == ip and f['operation'] == operation
                and f['destination'] == '10.99.0.66' and f['port'] == '18443', 'Unexpected emitted command/scope')
        return command

    def apply(command):
        # An explicit test action, separate from preview. Never eval or retry.
        time.sleep(.21)
        return cli(command)

    def applied(command):
        result = apply(command)
        require(result.returncode == 0 and fields(result.stdout).get('result') == 'APPLIED',
                'Mutation rejected/outcome unknown; do not retry: ' + result.stdout + result.stderr)

    def verify_process(label):
        pid = show(unit)['MainPID']
        check(label + '_actual_binary', sha(f'/proc/{pid}/exe') == FRESH_SHA and sha(HOST_BINARY) == HOST_SHA)
        check(label + '_isolated_namespace', os.stat(f'/proc/{pid}/ns/net').st_ino
              == os.stat('/run/netns/ns-veil-rogue').st_ino)
        threads = []
        for task in Path(f'/proc/{pid}/task').iterdir():
            f = dict(line.split(':', 1) for line in (task / 'status').read_text().splitlines() if ':' in line)
            threads.append({k: f[k].strip() for k in ['Uid', 'Groups', 'CapEff', 'CapPrm', 'CapInh', 'CapAmb', 'NoNewPrivs']})
        evidence.setdefault('processes', []).append(dict(label=label, pid=pid, sha256=sha(f'/proc/{pid}/exe'), threads=threads))
        check(label + '_all_threads_unprivileged', bool(threads) and all(
            all(int(v) == 979 for v in t['Uid'].split()) and t['Groups'] == '979' and t['NoNewPrivs'] == '1'
            and all(int(t[k], 16) == 0 for k in ['CapEff', 'CapPrm', 'CapInh', 'CapAmb']) for t in threads))

    profile = configparser.ConfigParser(interpolation=None)
    profile.optionxform = str
    profile.read_string(Path('/etc/systemd/system/veil-local-ban-pi-full.service').read_text())
    properties = {k: v for k, v in profile['Service'].items() if k not in
                  ['Type', 'User', 'Group', 'ExecStart', 'EnvironmentFile', 'ReadOnlyPaths', 'ReadWritePaths']}
    properties.update(User='veil-local-ban-pi-full', Group='veil-local-ban-pi-full',
        NetworkNamespacePath='/run/netns/ns-veil-rogue', BindReadOnlyPaths=f'{FRESH}:{HOST_BINARY}',
        ReadOnlyPaths=f'{runtime} /var/lib/veil-local-ban-pi-full /var/lib/veil-local-ban-pi-test /var/lib/veil-local-ban-pi-boot',
        ReadWritePaths=str(control))
    properties.update({k: v for k, v in profile['Unit'].items() if k.startswith('StartLimit')})
    command = ['systemd-run', '--unit=' + unit, '--service-type=notify', '--quiet']
    for k, v in properties.items():
        command += ['--property=' + k + '=' + v]
    command += [str(HOST_BINARY), 'serve', '--systemd-notify', '--iface', 'veth-rogue', '--ebpf-object', str(OBJECT),
                '--control-dir', str(control), '--destination', '10.99.0.66', '--port', '18443', '--protected-ip', '198.51.100.7']
    (root / 'command.json').write_text(json.dumps(command, indent=2))
    server = log = None
    attempted = False
    cert, key = root / 'cert.pem', root / 'test-key.pem'
    try:
        run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=isolated-operator',
             '-addext', 'subjectAltName=IP:10.99.0.66', '-keyout', key, '-out', cert])
        token = os.urandom(8).hex()
        context = ssl.create_default_context(cafile=str(cert))

        def https(port=18443):
            try:
                c = http.client.HTTPSConnection('10.99.0.66', port, context=context, timeout=1,
                                                source_address=('10.99.0.1', 0))
                try:
                    c.request('GET', '/')
                    response = c.getresponse()
                    return response.status == 200 and response.read() == token.encode()
                finally:
                    c.close()
            except (OSError, http.client.HTTPException):
                return False

        log = (root / 'https.log').open('xb')
        server = subprocess.Popen(['ip', 'netns', 'exec', 'ns-veil-rogue', '/usr/bin/python3', '-I', '-B',
                                   PREVIOUS / 'http-helper.py', 'server', cert, key, token], stdout=log, stderr=subprocess.STDOUT)
        check('normal_https_baseline', wait(https, 5) and https(18444))
        attempted = True
        run(command)
        check('ready_and_empty_list', show(unit)['ActiveState'] == 'active' and listing()[0]['entries'] == '0')
        verify_process('initial')
        original = status()
        ban = preview('ban')
        check('preview_is_read_only', listing()[0]['entries'] == '0' and status()['revision'] == original['revision']
              and status()['event_count'] == original['event_count'] and https())
        applied(ban)
        header, entries, text = listing()
        evidence['first_list'] = text
        check('preview_command_applies_and_list_agrees', len(entries) == 1 and entries[0]['entry'] == '10.99.0.1'
              and entries[0]['state'] == 'active' and entries[0]['confirmed'] == 'true' and not https()
              and https(18444) and int(status()['kernel_drop']) > 0)
        stale_unban = preview('unban')
        applied(preview('ban', '198.18.1.1'))
        rejected = apply(stale_unban)
        check('stale_revision_unban_refused_keeps_drop', rejected.returncode != 0
              and fields(rejected.stdout).get('code') == 'REVISION_CONFLICT' and not https())
        applied(preview('unban'))
        check('preview_unban_restores_https', https() and all(e['entry'] != '10.99.0.1' for e in listing()[1]))
        rejected = apply(ban)
        check('released_ban_old_command_refused', rejected.returncode != 0
              and fields(rejected.stdout).get('code') == 'REVISION_CONFLICT' and https())
        applied(preview('unban', '198.18.1.1'))
        for ip, ttl, expected in [('198.51.100.7', 30, 'PROTECTED_OR_INVALID_IP'),
                                  ('10.99.0.1', 0, 'INVALID_TTL')]:
            rejected = read('preview-ban', ip, ttl)
            check('preview_refused_' + expected, rejected.returncode != 0
                  and fields(rejected.stdout).get('code') == expected and 'execute_manually=' not in rejected.stdout)
        applied(preview('ban', ttl=3))
        deadline = int(listing()[1][0]['expires_at_boot_ns'])
        check('short_ban_live_before_pause', not https() and https(18444)
              and time.clock_gettime_ns(time.CLOCK_BOOTTIME) < deadline)
        run(['systemctl', 'kill', '--signal=SIGSTOP', '--kill-whom=main', unit])
        time.sleep(3.1)
        check('kernel_expiry_while_daemon_paused', https() and https(18444))
        run(['systemctl', 'kill', '--signal=SIGCONT', '--kill-whom=main', unit])
        check('bounded_cleanup', wait(lambda: status()['occupied'] == '0', 4)
              and listing()[0]['entries'] == '0' and int(status()['max_examined']) <= 16
              and int(status()['max_attempted']) <= 16)
        for i, ip in enumerate(['10.99.0.1', *[f'198.18.0.{n}' for n in range(1, 256)]]):
            applied(preview('ban', ip, 180))
            if i % 64 == 63:
                print(f'CAPACITY {i+1}/256', flush=True)
        header, entries, text = listing()
        evidence['full_list'] = text
        check('list_complete_256', header['entries'] == '256' and len(entries) == len({e['entry'] for e in entries}) == 256
              and all(e['state'] == 'active' and e['confirmed'] == 'true' for e in entries)
              and len(text.encode()) > 4096 and len(text.encode()) <= 65536)
        before_capacity = {e['entry']: e['expires_at_boot_ns'] for e in entries}
        rejected = read('preview-ban', '198.19.0.1', 180)
        check('preview_257_refused', rejected.returncode != 0 and fields(rejected.stdout).get('code') == 'CAPACITY'
              and 'execute_manually=' not in rejected.stdout)
        rejected = cli([str(HOST_BINARY), 'ban', '--control-dir', str(control), '--ip', '198.19.0.1', '--ttl-secs', '180',
                        '--instance', header['instance'], '--expected-revision', header['revision']])
        after_capacity = listing()
        check('direct_257_refused_no_eviction', rejected.returncode != 0 and fields(rejected.stdout).get('code') == 'CAPACITY'
              and before_capacity == {e['entry']: e['expires_at_boot_ns'] for e in after_capacity[1]}
              and after_capacity[0]['revision'] == header['revision'] and not https() and https(18444))
        old_ban, old_unban = preview('ban', ttl=180), preview('unban')
        run(['systemctl', 'restart', unit])
        current = listing()
        check('normal_restart_clears_256', current[0]['instance'] != header['instance'] and current[0]['entries'] == '0'
              and status()['occupied'] == '0' and https() and https(18444))
        for name, old in [('ban', old_ban), ('unban', old_unban)]:
            rejected = apply(old)
            check('old_instance_' + name + '_refused', rejected.returncode != 0
                  and fields(rejected.stdout).get('code') == 'INSTANCE_CONFLICT' and listing()[0]['entries'] == '0')
        verify_process('normal_restart')
        applied(preview('ban'))
        check('ban_before_sigkill', not https())
        old_instance, old_pid = status()['instance'], show(unit)['MainPID']
        run(['systemctl', 'kill', '--signal=SIGKILL', '--kill-whom=main', unit])
        check('sigkill_auto_restart_ready', wait(lambda: show(unit)['ActiveState'] == 'active'
              and show(unit)['MainPID'] not in ['0', old_pid]))
        check('sigkill_restart_empty', status()['instance'] != old_instance and listing()[0]['entries'] == '0'
              and https() and https(18444))
        verify_process('sigkill_restart')
        run(['systemctl', 'stop', unit])
        check('https_after_explicit_stop', not support.link().get('xdp', {}).get('attached') and https() and https(18444))
        evidence['complete'] = True
    except BaseException as error:
        evidence['error'] = repr(error)
        raise
    finally:
        if attempted:
            state = show(unit)
            if state['LoadState'] == 'loaded':
                require(state.get('Transient') == 'yes' and state.get('FragmentPath') == '/run/systemd/transient/' + unit,
                        'Preserve unknown unit')
                run(['systemctl', 'kill', '--signal=SIGCONT', '--kill-whom=main', unit], check=False)
                run(['systemctl', 'stop', unit])
            (root / 'unit-journal.log').write_text(run(['journalctl', '-u', unit, '--no-pager', '-o', 'short-iso']).stdout)
        if server is not None and server.poll() is None:
            server.terminate()
            server.wait(timeout=5)
        if log is not None:
            log.close()
        after = snapshot()
        (root / 'after.json').write_text(json.dumps(after, indent=2))
        compare = ['existing_fixture', 'new_fixture', 'extra_boot_fixture', 'service_file_hashes', 'services',
                   'mainnet', 'public_accounts', 'boot_id', 'prior_units', 'firewall', 'ufw']
        evidence['preservation'] = {k: before[k] == after[k] for k in compare}
        keys = ['ifindex', 'ifname', 'address', 'mtu', 'flags', 'xdp']
        normalize = lambda d: [{k: row.get(k) for k in keys} for row in json.loads(d['host_links']['stdout'])]
        evidence['preservation']['host_links'] = normalize(before) == normalize(after)
        evidence['preservation']['host_ipv4_routes'] = before['host_routes'] == after['host_routes']
        route_config = lambda d: [{k: v for k, v in r.items() if k not in ['expires', 'cache', 'used', 'age']}
                                  for r in d['host_ipv6_routes']]
        evidence['preservation']['host_ipv6_routes'] = route_config(before) == route_config(after)
        address_config = lambda d: [(r['ifname'], [(a.get('family'), a.get('local'), a.get('prefixlen'), a.get('scope'))
                                    for a in r.get('addr_info', [])]) for r in d['host_addresses']]
        evidence['preservation']['host_addresses'] = address_config(before) == address_config(after)
        evidence['preservation']['binaries'] = sha(FRESH) == FRESH_SHA and sha(HOST_BINARY) == HOST_SHA and sha(OBJECT) == OBJECT_SHA
        evidence['unit_after'] = show(unit)
        evidence['link_after'] = support.link()
        evidence['runtime_entries'] = sorted(p.name for p in control.iterdir())
        evidence['cleanup_ok'] = (not support.link().get('xdp', {}).get('attached')
            and evidence['unit_after']['ActiveState'] == 'inactive' and evidence['unit_after']['MainPID'] == '0'
            and not Path('/etc/systemd/system/multi-user.target.wants', unit).exists()
            and not run(['pgrep', '-u', '979'], check=False).stdout.strip()
            and not run(['ip', 'netns', 'pids', 'ns-veil-rogue']).stdout.strip()
            and not run(['ip', 'netns', 'exec', 'ns-veil-rogue', 'ss', '-Hlnt', '( sport = :18443 or sport = :18444 )']).stdout.strip())
        if not evidence['cleanup_ok'] or not all(evidence['preservation'].values()):
            evidence['functional_checks_passed'] = evidence['complete']
            evidence['complete'] = False
        save()
        print('ARTIFACTS=' + str(root), flush=True)
        require(evidence['cleanup_ok'] and all(evidence['preservation'].values()), 'Cleanup/preservation failed; inspect evidence')


if __name__ == '__main__':
    main()
