#!/usr/bin/env python3
"""Approved fixed pi-node boot acceptance. This driver never requests a reboot.

Each phase is single use. The external operator records/requests one reboot
between preboot and postboot. Unknown paths and partial installs are preserved.
"""
import argparse
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import pwd
import signal
import socket
import ssl
import stat
import subprocess
import sys
import time

NAME = 'veil-local-ban-pi-boot'
UNIT = NAME + '.service'
NET = 'veil-local-ban-pi-namespace.service'
OPT = Path('/opt') / NAME
STATE = Path('/var/lib') / NAME
PRIMARY = STATE / 'primary'
FRAGMENT = Path('/etc/systemd/system') / UNIT
NET_FRAGMENT = Path('/etc/systemd/system') / NET
ENABLE = Path('/etc/systemd/system/multi-user.target.wants') / UNIT
BINARY = Path('/opt/veil-local-ban-pi-full/veil-local-ban')
OBJECT = Path('/opt/veil-local-ban-pi-full/veil_xdp_kern.o')
BINARY_SHA = 'f9cf2dc594cc0808b91247a8f0d447edcb7b9370ae8273e0f777deebe85da05a'
OBJECT_SHA = '2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f'


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def run(args, check=True, timeout=20):
    p = subprocess.run([str(v) for v in args], capture_output=True, text=True, timeout=timeout)
    require(not check or p.returncode == 0, repr((args, p.returncode, p.stdout, p.stderr)))
    return p


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def boot_id():
    return Path('/proc/sys/kernel/random/boot_id').read_text().strip()


def exclusive(path, data, mode=0o600):
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, mode)
    with os.fdopen(fd, 'wb') as f:
        f.write(data); f.flush(); os.fchmod(f.fileno(), mode); os.fsync(f.fileno())


def fresh(path, mode, owner=None):
    path.mkdir(mode=mode); path.chmod(mode)
    if owner is not None:
        os.chown(path, owner.pw_uid, owner.pw_gid)


def show(unit=UNIT):
    return dict(v.split('=', 1) for v in run(['systemctl', 'show', unit]).stdout.splitlines() if '=' in v)


def link():
    return json.loads(run(['ip', '-j', '-details', '-n', 'ns-veil-rogue', 'link', 'show', 'veth-rogue']).stdout)[0]


def wait(predicate, seconds):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if predicate():
            return True
        time.sleep(.05)
    return False


def audit(work):
    result = json.loads(run(['/usr/bin/python3', '-B', work / 'audit-host.py']).stdout)
    result['boot_id'] = boot_id()
    result['kernel'] = os.uname().release
    result['host_ipv4'] = json.loads(run(['ip', '-j', '-4', 'address', 'show']).stdout)
    result['host_routes'] = json.loads(run(['ip', '-j', '-4', 'route', 'show']).stdout)
    result['services'] = {}
    result['service_file_hashes'] = {}
    for unit in ['ssh.service', 'docker.service', 'systemd-timesyncd.service', 'ufw.service', 'NetworkManager.service', 'pi-mainnet.service']:
        state = show(unit)
        result['services'][unit] = {k: state.get(k) for k in ['LoadState', 'ActiveState', 'SubState', 'UnitFileState', 'MainPID']}
        for path in [state.get('FragmentPath', ''), *state.get('DropInPaths', '').split()]:
            if path:
                result['service_file_hashes'][path] = sha(path)
    result['mainnet'] = json.loads(run(['docker', 'inspect', '--format',
        '{"id":"{{.Id}}","running":{{.State.Running}},"restart":"{{.HostConfig.RestartPolicy.Name}}","stop_timeout":{{.Config.StopTimeout}}}', 'mainnet']).stdout)
    return result


def validate(work):
    manifest = json.loads((OPT / 'installation.json').read_text())
    if manifest['driver_sha256'] != sha(__file__):
        # An explicit root-owned, append-only attestation preserves the first
        # driver and failed install result; never rewrite the install manifest.
        revision_path = work / 'driver-revision.json'
        info = revision_path.lstat()
        require(stat.S_ISREG(info.st_mode) and info.st_uid == 0 and stat.S_IMODE(info.st_mode) == 0o600
                and info.st_nlink == 1, 'Unknown driver revision attestation')
        revision = json.loads(revision_path.read_text())
        require(revision['previous'] == manifest['driver_sha256'] == sha(work / 'boot-driver.py')
                and revision['current'] == sha(__file__), 'Changed test driver')
    for path, digest in manifest['files'].items():
        require(sha(path) == digest, 'Changed installed file: ' + path)
    a = pwd.getpwnam('veil-local-ban-pi-full')
    require(a.pw_uid == a.pw_gid == 979 and a.pw_shell == '/usr/sbin/nologin', 'Changed service account')
    require(sha(BINARY) == BINARY_SHA and sha(OBJECT) == OBJECT_SHA, 'Changed product')
    return manifest


def enable_identity():
    s = ENABLE.lstat()
    require(stat.S_ISLNK(s.st_mode) and s.st_uid == 0 and os.readlink(ENABLE) == str(FRAGMENT), 'Unknown enable link')
    return dict(inode=s.st_ino, device=s.st_dev, target=os.readlink(ENABLE))


def cleanup(work):
    validate(work)
    run(['systemctl', 'stop', UNIT, NET])
    if os.path.lexists(ENABLE):
        require(enable_identity() == json.loads((work / 'enable-identity.json').read_text()), 'Preserve changed enable link')
        run(['systemctl', 'disable', UNIT])
    require(not os.path.lexists(ENABLE), 'Enable link remains')
    for unit in [UNIT, NET]:
        s = show(unit)
        require(s['ActiveState'] == 'inactive' and s['MainPID'] == '0' and s['UnitFileState'] == 'disabled', 'Unit not stopped/disabled')
    require(not link().get('xdp', {}).get('attached'), 'XDP remains')


def install(work, evidence):
    targets = [OPT, STATE, FRAGMENT, NET_FRAGMENT, ENABLE]
    targets += [Path(str(p) + suffix) for p in [FRAGMENT, NET_FRAGMENT] for suffix in ['.d', '.disabled']]
    require(all(not os.path.lexists(p) for p in targets), 'Preserve existing installation/override')
    require(not os.path.lexists(ENABLE.with_name(NET)), 'Unexpected namespace enable link')
    for unit in [UNIT, NET]:
        s = show(unit)
        require(s['LoadState'] == 'not-found' and not s.get('DropInPaths'), 'Existing unit')
    expected = json.loads((work / 'bundle-sha256.json').read_text())
    require(all(sha(work / 'bundle' / p) == h for p, h in expected.items()), 'Bundle changed')
    require(sha(BINARY) == BINARY_SHA and sha(OBJECT) == OBJECT_SHA, 'Product changed')
    a = pwd.getpwnam('veil-local-ban-pi-full')
    require(a.pw_uid == a.pw_gid == 979 and a.pw_shell == '/usr/sbin/nologin', 'Wrong account')
    require(not run(['pgrep', '-u', '979'], check=False).stdout.strip(), 'Existing service UID process')
    require(not run(['ip', 'netns', 'exec', 'ns-veil-rogue', 'ss', '-Hlnt', '( sport = :18443 or sport = :18444 )']).stdout.strip(), 'Existing listener')
    p = run(['/usr/bin/python3', '-I', '-B', work / 'bundle/namespace.py', 'inspect'])
    require(json.loads(p.stdout) == dict(result='verified_existing', changed=False), 'Existing namespace changed')
    before = audit(work)
    exclusive(work / 'before.json', json.dumps(before, indent=2).encode())
    fresh(OPT, 0o755); fresh(STATE, 0o755); fresh(PRIMARY, 0o700, a)
    mappings = [('namespace.py', OPT / 'namespace.py', 0o555), ('service.conf', OPT / 'service.conf', 0o600),
                (NET, NET_FRAGMENT, 0o644), (UNIT, FRAGMENT, 0o644)]
    for source, target, mode in mappings:
        exclusive(target, (work / 'bundle' / source).read_bytes(), mode)
    p = run(['systemd-analyze', 'verify', FRAGMENT, NET_FRAGMENT])
    exclusive(work / 'unit-verify.log', (p.stdout + p.stderr).encode())
    manifest = dict(driver_sha256=sha(__file__), files={str(p): sha(p) for _, p, _ in mappings})
    exclusive(OPT / 'installation.json', json.dumps(manifest, indent=2).encode())
    run(['systemctl', 'daemon-reload'])
    for unit, path in [(UNIT, FRAGMENT), (NET, NET_FRAGMENT)]:
        s = show(unit)
        require(s['FragmentPath'] == str(path) and not s.get('DropInPaths') and s['Transient'] == 'no'
                and s['ActiveState'] == 'inactive' and s['UnitFileState'] == 'disabled', 'Unexpected installed unit')
    evidence['installed'] = manifest


def acceptance(work, evidence, postboot):
    validate(work)
    # Reuse only pure wire-client text from the previously verified driver.
    spec = importlib.util.spec_from_file_location('wire_client', work / 'permanent-driver.py')
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    server = None
    def check(name, condition):
        evidence['checks'].append(dict(name=name, passed=bool(condition)))
        print(('PASS ' if condition else 'FAIL ') + name, flush=True)
        require(condition, name)
    def client(op=0, ttl=0, instance=None, revision=0, ip='10.99.0.1'):
        data = json.dumps(dict(op=op, ttl=ttl, instance=instance, revision=revision, ip=ip))
        return json.loads(run(['setpriv', '--reuid=979', '--regid=979', '--clear-groups', '/usr/bin/python3', '-c',
                              module.CLIENT, PRIMARY / 'control.sock', data], timeout=5).stdout)
    def mutate(op=1, ttl=0, ip='10.99.0.1'):
        s = client(); time.sleep(.21)
        return client(op, ttl, s['instance'], int(s['revision']), ip)
    def caps():
        pid = show()['MainPID']; rows = []
        for task in Path(f'/proc/{pid}/task').iterdir():
            f = dict(v.split(':', 1) for v in (task / 'status').read_text().splitlines() if ':' in v)
            rows.append({k: f[k].strip() for k in ['Uid', 'Groups', 'CapEff', 'CapPrm', 'CapInh', 'CapAmb', 'NoNewPrivs']})
        evidence['caps'] = rows
        check('uid_979_all_threads_runtime_caps_zero', bool(rows) and all(
            all(int(v) == 979 for v in r['Uid'].split()) and r['Groups'] == '979' and r['NoNewPrivs'] == '1'
            and all(int(r[k], 16) == 0 for k in ['CapEff', 'CapPrm', 'CapInh', 'CapAmb']) for r in rows))
    success = False
    try:
        if postboot:
            old = json.loads((work / 'preboot-result.json').read_text())
            check('boot_id_changed', boot_id() != old['boot_id'])
            check('automatically_active_before_any_start_command', wait(lambda: show()['ActiveState'] == 'active', 8))
            s, n = show(), show(NET)
            evidence['auto_unit'], evidence['auto_namespace_unit'] = s, n
            check('auto_enabled_dependency_ready_and_ordered', s['UnitFileState'] == 'enabled' and n['ActiveState'] == 'active'
                  and n['Result'] == 'success' and int(n['ActiveEnterTimestampMonotonic']) <= int(s['ExecMainStartTimestampMonotonic']))
            journal = run(['journalctl', '-b', '-u', NET, '--no-pager', '-o', 'cat']).stdout
            evidence['namespace_boot_journal'] = journal
            check('namespace_actually_restored_this_boot', '"result": "restored", "changed": true' in journal)
            pid = s['MainPID']
            check('daemon_in_restored_namespace', os.stat(f'/proc/{pid}/ns/net').st_ino == os.stat('/run/netns/ns-veil-rogue').st_ino
                  and os.stat(f'/proc/{pid}/ns/net').st_ino != os.stat('/proc/1/ns/net').st_ino)
        else:
            check('fresh_primary', not list(PRIMARY.iterdir()))
            run(['systemctl', 'start', UNIT])
            check('dependency_and_daemon_active', show()['ActiveState'] == 'active' and show(NET)['ActiveState'] == 'active')
        first = client(); evidence['first_status'] = first
        check('empty_256_contract', first.get('capacity') == '256' and first.get('occupied') == '0')
        if postboot:
            check('new_instance_after_host_reboot', first['instance'] != old['final_status']['instance'])
            stale = client(1, 30, old['final_status']['instance'], int(old['final_status']['revision']))
            check('old_instance_replay_refused', stale.get('code') == 'INSTANCE_CONFLICT' and client()['occupied'] == '0')
        caps()
        cert, key = work / 'cert.pem', work / 'test-key.pem'
        if not postboot:
            run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=isolated-boot-test',
                 '-addext', 'subjectAltName=IP:10.99.0.66', '-keyout', key, '-out', cert])
            exclusive(work / 'https-body.txt', os.urandom(8).hex().encode())
        token = (work / 'https-body.txt').read_text()
        context = ssl.create_default_context(cafile=str(cert))
        def https(port=18443):
            try:
                c = http.client.HTTPSConnection('10.99.0.66', port, context=context, timeout=1, source_address=('10.99.0.1', 0))
                try:
                    c.request('GET', '/'); r = c.getresponse(); return r.status == 200 and r.read() == token.encode()
                finally:
                    c.close()
            except (OSError, http.client.HTTPException):
                return False
        label = 'postboot' if postboot else 'preboot'
        with (work / (label + '-https.log')).open('xb') as log:
            server = subprocess.Popen(['ip', 'netns', 'exec', 'ns-veil-rogue', '/usr/bin/python3', work / 'http-helper.py',
                                       'server', cert, key, token], stdout=log, stderr=subprocess.STDOUT)
            check('normal_https_both_ports', wait(https, 5) and https(18444))
            if postboot:
                check('ban_scope', mutate(ttl=3)['ok'] and not https() and https(18444))
                check('kernel_ttl_expiry', wait(https, 5))
                check('bounded_cleanup', wait(lambda: client()['occupied'] == '0', 4))
                check('individual_unban', mutate(ttl=30)['ok'] and not https() and mutate(2)['ok'] and https())
            else:
                check('first_actual_source_ban', mutate(ttl=3600)['ok'])
                for i in range(255):
                    require(mutate(ttl=3600, ip=f'198.18.0.{i}')['ok'], f'Capacity input {i}')
                    if i % 64 == 63: print(f'CAPACITY {i+2}/256', flush=True)
                check('full_256', client()['occupied'] == '256')
                check('257_rejected_without_eviction', mutate(ttl=3600, ip='198.19.0.1').get('code') == 'CAPACITY'
                      and not https() and https(18444))
                require(not os.path.lexists(ENABLE), 'Existing enable link')
                run(['systemctl', 'enable', UNIT])
                exclusive(work / 'enable-identity.json', json.dumps(enable_identity()).encode())
                check('only_owned_boot_unit_enabled', show()['UnitFileState'] == 'enabled'
                      and show(NET)['UnitFileState'] == 'disabled' and not os.path.lexists(ENABLE.with_name(NET)))
            evidence['final_status'] = client(); evidence['boot_id'] = boot_id()
            evidence['completed_wallclock_ns'] = time.time_ns()
            success = True
    finally:
        if server is not None and server.poll() is None:
            server.terminate(); server.wait(timeout=5)
        if postboot or not success:
            cleanup(work)
            evidence['cleanup_ok'] = True
    # Preboot deliberately leaves the owned daemon running with 256 live BANs.
    # Its HTTP child is stopped; postboot makes a fresh child in the restored ns.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase', choices=['install', 'preboot', 'postboot', 'cleanup'])
    parser.add_argument('--workdir', required=True); parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    require(os.geteuid() == 0 and args.apply, 'Root and explicit --apply required; human approval is separate')
    work = Path(args.workdir).resolve()
    require(work.parent == Path('/home/maakie') and work.name.startswith('vlb-boot-') and work.is_dir(), 'Unexpected staging')
    os.umask(0o077)
    exclusive(work / (args.phase + '-started.json'), json.dumps(dict(boot_id=boot_id(), phase=args.phase)).encode())
    evidence = dict(phase=args.phase, checks=[], complete=False, driver_sha256=sha(__file__), boot_id=boot_id())
    try:
        if args.phase == 'install': install(work, evidence)
        elif args.phase == 'cleanup': cleanup(work)
        else: acceptance(work, evidence, args.phase == 'postboot')
        evidence['complete'] = True
    except BaseException as error:
        evidence['error'] = repr(error)
        raise
    finally:
        exclusive(work / (args.phase + '-result.json'), json.dumps(evidence, indent=2).encode())
        print('RESULT=' + str(work / (args.phase + '-result.json')), flush=True)


if __name__ == '__main__':
    main()
