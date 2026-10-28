#!/usr/bin/env python3
"""Host-independent acceptance in a transient, disconnected network namespace.

The functional sequence is copied unchanged from the existing operator test.
No account DB, permanent unit, host interface/route/firewall or EFI changes.
"""
import configparser
import hashlib
import http.client
import json
import os
from pathlib import Path
import pwd
import shlex
import ssl
import subprocess
import sys
import time


def require(ok, message):
    if not ok:
        raise RuntimeError(message)


def run(args, check=True, timeout=20):
    p = subprocess.run(list(map(str, args)), capture_output=True, text=True, timeout=timeout)
    require(not check or p.returncode == 0, repr((args, p.returncode, p.stdout, p.stderr)))
    return p


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def show(unit):
    keys = 'LoadState,ActiveState,SubState,MainPID,Transient,FragmentPath,UnitFileState'
    return dict(line.split('=', 1) for line in run(['systemctl', 'show', unit, '--property=' + keys]).stdout.splitlines() if '=' in line)


def wait(predicate, seconds=8):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if predicate():
            return True
        time.sleep(.05)
    return False


def fields(text):
    result = {}
    for line in text.splitlines()[1:]:
        if '=' in line and not line.startswith('entry='):
            key, value = line.split('=', 1)
            require(key not in result, 'Duplicate field: ' + key)
            result[key] = value
    return result


def rows(text):
    return [dict(part.split('=', 1) for part in line.split(','))
            for line in text.splitlines() if line.startswith('entry=')]


def holder(root):
    print('PRIVATE_NET_READY', flush=True)
    require(sys.stdin.readline().strip() == 'GO', 'No explicit server start')
    import http.server
    import threading
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = (root / 'token').read_bytes()
            self.send_response(200)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *_):
            pass
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(root / 'cert.pem', root / 'test-key.pem')
    for port in (18443, 18444):
        server = http.server.ThreadingHTTPServer(('10.99.0.66', port), Handler)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
    while True:
        time.sleep(1)


def main(root, runner):
    require(os.geteuid() == 0, 'Root required')
    require(os.stat('/proc/self/ns/net').st_ino != os.stat('/proc/1/ns/net').st_ino,
            'Refuse to run network setup in the host namespace')
    require([r['ifname'] for r in json.loads(run(['ip', '-j', 'link']).stdout)] == ['lo'],
            'Require a fresh private namespace')
    root = root.resolve(strict=True)
    evidence_dir = root / 'evidence'
    control = root / 'control'
    HOST_BINARY = root / 'veil-local-ban'
    OBJECT = root / 'veil_xdp_kern.o'
    FRESH_SHA = json.loads((root / 'scope.json').read_text())['binary_sha256']
    require(root.parent == Path('/run') and root.name == 'veil-gcp-generic-20261027', 'Wrong new trial root')
    OBJECT_SHA = '2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f'
    require(sha(HOST_BINARY) == FRESH_SHA and sha(OBJECT) == OBJECT_SHA, 'Unexpected artifacts')
    unit = root.name + '-daemon.service'
    username = root.name
    require(show(unit)['LoadState'] == 'not-found', 'Preserve existing unit')
    try:
        pwd.getpwnam(username)
    except KeyError:
        pass
    else:
        raise RuntimeError('Refuse existing account')
    evidence = dict(complete=False, checks=[], cli=[], unit=unit, runner=runner,
                    binary_sha256=FRESH_SHA, object_sha256=OBJECT_SHA,
                    driver_sha256=sha(__file__), runtime_dir=str(root))
    server = None
    log = None
    attempted = False
    def save():
        (evidence_dir / 'result.json').write_text(json.dumps(evidence, indent=2))
    def check(name, ok):
        evidence['checks'].append(dict(name=name, passed=bool(ok)))
        save()
        print(('PASS ' if ok else 'FAIL ') + name, flush=True)
        require(ok, name)
    def link():
        return json.loads(run(['nsenter', '--target', server.pid, '--net', '--',
                               'ip', '-j', '-details', 'link', 'show', 'veth-rogue']).stdout)[0]
    def credentials():
        pid = show(unit)['MainPID']
        require(pid != '0', 'No daemon PID')
        data = dict(l.split(':', 1) for l in Path('/proc/' + pid + '/status').read_text().splitlines() if ':' in l)
        return int(data['Uid'].split()[1]), int(data['Gid'].split()[1])
    def cli(arguments):
        uid, gid = credentials()
        p = run(['setpriv', '--reuid=' + str(uid), '--regid=' + str(gid), '--clear-groups',
                 '--no-new-privs', *map(str, arguments)], check=False, timeout=5)
        evidence['cli'].append(dict(argv=list(map(str, arguments)), exit_code=p.returncode,
                                    stdout=p.stdout, stderr=p.stderr))
        return p
    def read(operation, ip=None, ttl=None):
        command = [str(HOST_BINARY), operation, '--control-dir', str(control)]
        if ip is not None:
            command += ['--ip', ip]
        if ttl is not None:
            command += ['--ttl-secs', str(ttl)]
        end = time.monotonic() + 2
        while True:
            p = cli(command)
            f = fields(p.stdout)
            if not (f.get('code') == 'BUSY' or f.get('policy_busy') == 'true') or time.monotonic() >= end:
                return p
            time.sleep(.01)
    def status():
        p = read('status')
        require(p.returncode == 0 and 'revision' in fields(p.stdout), 'Status unavailable')
        return fields(p.stdout)
    def listing():
        p = read('list')
        require(p.returncode == 0, 'List refused: ' + p.stdout)
        return fields(p.stdout), rows(p.stdout), p.stdout
    def preview(operation, ip='10.99.0.1', ttl=30):
        p = read('preview-' + operation, ip, ttl if operation == 'ban' else None)
        require(p.returncode == 0, 'Preview refused: ' + p.stdout)
        f = fields(p.stdout)
        command = shlex.split(f['execute_manually'])
        expected = [str(HOST_BINARY), operation, '--control-dir', str(control), '--ip', ip]
        if operation == 'ban':
            expected += ['--ttl-secs', str(ttl)]
        expected += ['--instance', f['instance'], '--expected-revision', f['revision']]
        require(command == expected and f['ip'] == ip and f['operation'] == operation
                and f['destination'] == '10.99.0.66' and f['port'] == '18443', 'Unexpected preview scope')
        return command
    def apply(command):
        time.sleep(.21)
        return cli(command)
    def applied(command):
        p = apply(command)
        require(p.returncode == 0 and fields(p.stdout).get('result') == 'APPLIED',
                'Mutation refused/outcome unknown; never retry: ' + p.stdout + p.stderr)
    def verify_process(label):
        pid = show(unit)['MainPID']
        uid, gid = credentials()
        check(label + '_actual_binary', sha('/proc/' + pid + '/exe') == FRESH_SHA)
        check(label + '_isolated_namespace', os.stat('/proc/' + pid + '/ns/net').st_ino
              == os.stat('/proc/' + str(server.pid) + '/ns/net').st_ino
              != os.stat('/proc/1/ns/net').st_ino)
        threads = []
        for task in Path('/proc/' + pid + '/task').iterdir():
            f = dict(l.split(':', 1) for l in (task / 'status').read_text().splitlines() if ':' in l)
            threads.append({k: f[k].strip() for k in ['Uid', 'Gid', 'Groups', 'CapEff', 'CapPrm', 'CapInh', 'CapAmb', 'NoNewPrivs']})
        evidence.setdefault('processes', []).append(dict(label=label, pid=pid, uid=uid, gid=gid, threads=threads))
        check(label + '_all_threads_unprivileged', uid != 0 and gid != 0 and bool(threads) and all(
            all(int(v) == uid for v in t['Uid'].split()) and t['Groups'] == str(gid) and t['NoNewPrivs'] == '1'
            and all(int(t[k], 16) == 0 for k in ['CapEff', 'CapPrm', 'CapInh', 'CapAmb']) for t in threads))
    profile = configparser.ConfigParser(interpolation=None)
    profile.optionxform = str
    profile.read(root / 'profile.service')
    properties = {k: v for k, v in profile['Service'].items() if k not in
                  ['Type', 'User', 'Group', 'ExecStart', 'EnvironmentFile', 'ReadOnlyPaths', 'ReadWritePaths']}
    properties.update(DynamicUser='yes', User=username, Group=username, BindsTo=runner, After=runner,
                      ReadOnlyPaths=str(root), ReadWritePaths=str(control),
                      ExecStartPre='+/usr/bin/chown --no-dereference ' + username + ':' + username + ' ' + str(control))
    properties.update({k: v for k, v in profile['Unit'].items() if k.startswith('StartLimit')})
    try:
        cert, key = evidence_dir / 'cert.pem', evidence_dir / 'test-key.pem'
        run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=isolated-hp',
             '-addext', 'subjectAltName=IP:10.99.0.66', '-keyout', key, '-out', cert])
        token = os.urandom(8).hex()
        (evidence_dir / 'token').write_text(token)
        log = (evidence_dir / 'https.log').open('xb')
        server = subprocess.Popen(['unshare', '--net', '--', '/usr/bin/python3', '-I', '-B', __file__,
                                   'holder', str(evidence_dir)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=log, text=True)
        require(server.stdout.readline().strip() == 'PRIVATE_NET_READY', 'Server namespace not ready')
        require(os.stat('/proc/' + str(server.pid) + '/ns/net').st_ino
                != os.stat('/proc/self/ns/net').st_ino, 'Separate namespaces required')
        run(['ip', 'link', 'add', 'veth-client', 'type', 'veth', 'peer', 'name', 'veth-rogue'])
        run(['ip', 'link', 'set', 'veth-rogue', 'netns', server.pid])
        for command in [['link', 'set', 'lo', 'up'], ['address', 'add', '10.99.0.1/24', 'dev', 'veth-client'],
                        ['link', 'set', 'veth-client', 'up']]:
            run(['ip', *command])
        for command in [['link', 'set', 'lo', 'up'], ['address', 'add', '10.99.0.66/24', 'dev', 'veth-rogue'],
                        ['link', 'set', 'veth-rogue', 'up']]:
            run(['nsenter', '--target', server.pid, '--net', '--', 'ip', *command])
        properties['NetworkNamespacePath'] = '/proc/' + str(server.pid) + '/ns/net'
        command = ['systemd-run', '--unit=' + unit, '--service-type=notify', '--quiet']
        for k, v in properties.items():
            command += ['--property=' + k + '=' + v]
        command += [str(HOST_BINARY), 'serve', '--systemd-notify', '--iface', 'veth-rogue', '--xdp-mode', 'generic', '--ebpf-object', str(OBJECT),
                    '--control-dir', str(control), '--destination', '10.99.0.66', '--port', '18443', '--protected-ip', '198.51.100.7']
        (evidence_dir / 'command.json').write_text(json.dumps(command, indent=2))
        check('fresh_link_no_existing_xdp', not link().get('xdp', {}).get('attached'))
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
        server.stdin.write('GO\n')
        server.stdin.flush()
        # Existing functional checks are appended here by the fixture generator.
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
        check('sigkill_detaches_before_restart', wait(lambda: not link().get('xdp', {}).get('attached'), 5))
        run(['systemctl', 'start', unit])
        check('sigkill_explicit_restart_ready', wait(lambda: show(unit)['ActiveState'] == 'active'
              and show(unit)['MainPID'] not in ['0', old_pid]))
        check('sigkill_restart_empty', status()['instance'] != old_instance and listing()[0]['entries'] == '0'
              and https() and https(18444))
        verify_process('sigkill_restart')
        run(['systemctl', 'stop', unit])
        check('https_after_explicit_stop', not link().get('xdp', {}).get('attached') and https() and https(18444))
        evidence['complete'] = True
    except BaseException as error:
        evidence['error'] = repr(error)
        raise
    finally:
        try:
            if attempted:
                state = show(unit)
                if state['LoadState'] == 'loaded':
                    require(state.get('Transient') == 'yes' and state.get('FragmentPath') == '/run/systemd/transient/' + unit,
                            'Preserve unknown unit')
                    run(['systemctl', 'kill', '--signal=SIGCONT', '--kill-whom=main', unit], check=False)
                    run(['systemctl', 'stop', unit])
                (evidence_dir / 'unit-journal.log').write_text(run(['journalctl', '-u', unit, '--no-pager', '-o', 'short-iso']).stdout)
            evidence['unit_after'] = show(unit)
            if server is not None and server.poll() is None:
                evidence['link_after'] = link()
            evidence['runtime_entries'] = sorted(p.name for p in control.iterdir())
            # Revoke the recycled dynamic UID's access to this retained fixture.
            os.chown(control, 0, 0)
            os.chmod(control, 0o700)
            try:
                pwd.getpwnam(username)
                released = False
            except KeyError:
                released = True
            evidence['dynamic_account_released'] = released
            evidence['cleanup_ok'] = (not evidence.get('link_after', {}).get('xdp', {}).get('attached')
                and evidence['unit_after']['ActiveState'] == 'inactive' and evidence['unit_after']['MainPID'] == '0'
                and not Path('/etc/systemd/system/multi-user.target.wants', unit).exists() and released)
        finally:
            if server is not None and server.poll() is None:
                server.terminate()
                server.wait(timeout=5)
            if log is not None:
                log.close()
            evidence['server_reaped'] = server is None or server.poll() is not None
            if not evidence.get('cleanup_ok', False) or not evidence['server_reaped']:
                evidence['functional_checks_passed'] = evidence['complete']
                evidence['complete'] = False
            save()
            print('ARTIFACTS=' + str(evidence_dir), flush=True)
        require(evidence['cleanup_ok'] and evidence['server_reaped'], 'Cleanup failed; preserve evidence')


if __name__ == '__main__':
    if sys.argv[1] == 'holder':
        holder(Path(sys.argv[2]))
    else:
        main(Path(sys.argv[1]), sys.argv[2])
