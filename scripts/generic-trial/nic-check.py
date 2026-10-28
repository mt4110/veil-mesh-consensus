#!/usr/bin/env python3
"""Disposable GCP trial only. Mutations require pinned package + VM ID + boot.

No cloud API, network-rule edits, default-mode attachment or BAN command.
Offline tests mock every side effect. Do not execute live before approval.
"""
import argparse
import configparser
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import time

ROOT = Path('/run/veil-gcp-generic-20261027')
IFACE = 'ens4'
VM = 'vlb-gcp-generic-20261027-target'


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


features = load('trial_features', 'features.py')
access = load('trial_access', 'access-check.py')
require = features.require


def run(args, check=True):
    p = subprocess.run(list(map(str, args)), capture_output=True, text=True, timeout=20)
    require(len(p.stdout) + len(p.stderr) <= 131072, 'Reply outside bound')
    require(not check or p.returncode == 0, 'Command failed: ' + repr(args) + ': ' + p.stderr[:1000])
    return p


def save(name, data):
    require(re.fullmatch(r'[a-z0-9-]+\.json', name), 'Fixed evidence name required')
    fd = os.open(ROOT / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as out:
        json.dump(data, out, indent=2)
        out.write('\n')


def read(name):
    p = ROOT / name
    st = p.lstat()
    require(stat.S_ISREG(st.st_mode) and st.st_uid == 0 and st.st_nlink == 1
            and stat.S_IMODE(st.st_mode) == 0o600 and st.st_size <= 65536, 'Unsafe owner record')
    fd = os.open(p, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as stream:
        require(access.stable(os.fstat(stream.fileno())) == access.stable(st), 'Record changed')
        value = json.load(stream)
        require(access.stable(os.fstat(stream.fileno())) == access.stable(st), 'Record changed while reading')
    return value


def metadata():
    result = {}
    for name, path in [('id', 'instance/id'), ('name', 'instance/name'),
                       ('project', 'project/project-id'), ('zone', 'instance/zone')]:
        c = http.client.HTTPConnection('169.254.169.254', timeout=2)
        try:
            c.request('GET', '/computeMetadata/v1/' + path, headers={'Metadata-Flavor': 'Google'})
            r = c.getresponse()
            require(r.status == 200 and r.getheader('Metadata-Flavor') == 'Google', 'GCE metadata required')
            data = r.read(4097)
            require(len(data) <= 4096, 'Metadata too long')
            result[name] = data.decode().strip()
        finally:
            c.close()
    require(result['id'].isdigit() and result['name'] == VM and result['project'] == 'veil-x-tests'
            and result['zone'].endswith('/zones/us-central1-b'), 'Wrong disposable VM')
    return result


def show(unit):
    keys = 'LoadState,ActiveState,MainPID,Transient,FragmentPath,ExecMainStartTimestampMonotonic'
    return dict(row.split('=', 1) for row in run(['systemctl', 'show', unit, '-p', keys]).stdout.splitlines() if '=' in row)


def vacant(link):
    require(not link.get('xdp', {}).get('attached') and not link.get('xdp', {}).get('prog'), 'Existing XDP preserved')


def capture():
    require(os.stat('/proc/self/ns/net').st_ino == os.stat('/proc/1/ns/net').st_ino, 'Host netns required for this NIC helper')
    link = json.loads(run(['ip', '-j', '-d', 'link', 'show', 'dev', IFACE]).stdout)
    require(len(link) == 1, 'Ambiguous interface')
    link = link[0]
    require(link['ifname'] == IFACE and link['mtu'] == 1460 and not link.get('master')
            and not link.get('linkinfo'), 'Unsupported dedicated NIC topology')
    driver = (Path('/sys/class/net') / IFACE / 'device/driver').resolve().name
    require(driver == 'virtio_net', 'Expected VirtIO driver')
    addr = json.loads(run(['ip', '-j', '-4', 'address', 'show', 'dev', IFACE]).stdout)[0]['addr_info']
    require([a['local'] for a in addr if a['scope'] == 'global'] == ['10.77.28.10'], 'Wrong destination')
    routes = json.loads(run(['ip', '-j', '-4', 'route', 'show', 'default']).stdout)
    require(len(routes) == 1 and routes[0]['dev'] == IFACE, 'Ambiguous route')
    qdiscs = json.loads(run(['tc', '-j', 'qdisc', 'show', 'dev', IFACE]).stdout)
    require(len(qdiscs) <= 16, 'Qdisc bound')
    qstable = []
    handles = set()
    for q in qdiscs:
        require(q['kind'] in ('noqueue', 'mq', 'fq_codel', 'pfifo_fast', 'fq'), 'Preserve unknown TC')
        require(re.fullmatch(r'[0-9a-fA-F]+:', q['handle']) is not None, 'Unknown qdisc handle')
        handles.add(q['handle'])
        qstable.append({k: q[k] for k in ('kind', 'handle', 'parent', 'root') if k in q})
    for handle in handles:
        require(json.loads(run(['tc', '-j', 'filter', 'show', 'dev', IFACE, 'parent', handle]).stdout) == [], 'Existing TC filter')
    identity = {'boot': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
                'netns': os.stat('/proc/self/ns/net').st_ino, 'ifindex': link['ifindex'],
                'iface': IFACE, 'mtu': link['mtu'], 'mac': link['address'], 'driver': driver,
                'routes': [{k: r[k] for k in ('dst', 'dev', 'gateway', 'prefsrc', 'metric', 'table') if k in r} for r in routes],
                'qdiscs': qstable}
    return {'identity': identity, 'features': features.get_features(IFACE), 'link': link}


def context(manifest, cleanup=False):
    require(os.geteuid() == 0, 'Root trial orchestration required')
    m = access.check_files(ROOT, manifest)
    scope = json.loads((ROOT / 'scope.json').read_text())
    require(scope['binary_sha256'] == m['files']['veil-local-ban']['sha256']
            and scope['instance'] == VM and scope['xdp_mode'] == 'generic'
            and scope['management_source'] == '111.217.200.148', 'Wrong pinned scope')
    state = read('session.json')
    require(metadata() == state['metadata'] and state['boot'] == Path('/proc/sys/kernel/random/boot_id').read_text().strip(), 'VM ID/boot changed')
    elapsed = time.time() - state['t0']
    require(elapsed >= 0 and (cleanup or elapsed < 2400), 'Outside trial window')
    return state


def before_set(manifest, restore=False):
    state = context(manifest, cleanup=restore)
    current = capture()
    vacant(current['link'])
    base = read('b0.json')
    require(current['identity'] == base['identity'], 'NIC identity/topology changed')
    for case in ('normal', 'kill'):
        unit = ROOT.name + '-nic-' + case + '.service'
        require(show(unit)['MainPID'] == '0', 'Stop all NIC daemons before feature SET')
    return state, base, current


def prepare(manifest):
    state, base, current = before_set(manifest)
    require(time.time() - state['t0'] < 1500, 'No new prepare after 25 minutes')
    args = features.off_args(base['features'], current['features'])
    cmd = features.set_command(args)
    save('prepare-attempt.json', {'command': cmd, 'identity': current['identity']})
    if cmd:
        run(cmd)
    after = capture()
    require(after['identity'] == base['identity'], 'NIC identity changed during SET')
    features.off_result(base['features'], after['features'])
    save('b1.json', after)
    return {'prepared': True, 'command': cmd}


def restore(manifest):
    _, base, current = before_set(manifest, restore=True)
    if not (ROOT / 'prepare-attempt.json').exists():
        require(current['features'] == base['features'], 'No owned change to restore')
        save('restored.json', current)
        return {'restored': True, 'command': None}
    attempt = read('prepare-attempt.json')
    require(attempt['identity'] == base['identity'], 'Missing matching prepare ownership')
    cmd = features.set_command(features.restore_args(base['features'], current['features']))
    save('restore-attempt.json', {'command': cmd})
    if cmd:
        run(cmd)
    after = capture()
    require(after['identity'] == base['identity'] and after['features'] == base['features'], 'B0 not restored; delete owned VM, do not retry SET')
    save('restored.json', after)
    return {'restored': True, 'command': cmd}


def unit_for(case):
    require(case in ('normal', 'kill'), 'Only two fixed cases')
    return ROOT.name + '-nic-' + case + '.service'


def require_case_start(case, elapsed, normal_stopped):
    require((case == 'normal' and elapsed < 1500) or (case == 'kill' and elapsed < 1920 and normal_stopped),
            'Case order/time bound; no failure retry')


def start(manifest, case):
    state = context(manifest)
    require_case_start(case, time.time() - state['t0'], (ROOT / 'normal-stopped.json').exists()
                       and (ROOT / 'normal-verified.json').exists())
    b1, current = read('b1.json'), capture()
    vacant(current['link'])
    require(current['identity'] == b1['identity'] and current['features'] == b1['features'], 'B1 drift')
    features.admit_off(current['features'])
    unit = unit_for(case)
    require(show(unit)['LoadState'] == 'not-found', 'Preserve existing unit')
    control = ROOT / ('control-' + case)
    control.mkdir(mode=0o700)
    profile = configparser.ConfigParser(interpolation=None)
    profile.optionxform = str
    profile.read(ROOT / 'profile.service')
    props = {k: v for k, v in profile['Service'].items() if k not in ('Type', 'User', 'Group', 'ExecStart', 'EnvironmentFile', 'ReadOnlyPaths', 'ReadWritePaths')}
    user = ROOT.name + ('-n' if case == 'normal' else '-k')
    props.update(DynamicUser='yes', User=user, Group=user, Restart='no', RuntimeMaxSec='180s',
                 ReadOnlyPaths=str(ROOT), ReadWritePaths=str(control),
                 ExecStartPre='+/usr/bin/chown --no-dereference ' + user + ':' + user + ' ' + str(control))
    cmd = ['systemd-run', '--unit=' + unit, '--service-type=notify', '--quiet']
    cmd += ['--property=' + k + '=' + v for k, v in props.items()]
    cmd += [str(ROOT / 'veil-local-ban'), 'serve', '--systemd-notify', '--iface', IFACE,
            '--xdp-mode', 'generic', '--ebpf-object', str(ROOT / 'veil_xdp_kern.o'),
            '--control-dir', str(control), '--destination', '10.77.28.10', '--port', '18443',
            '--protected-ip', '111.217.200.148']
    save(case + '-attempt.json', {'unit': unit, 'command': cmd})
    run(cmd)
    active = show(unit)
    require(active['Transient'] == 'yes' and active['FragmentPath'] == '/run/systemd/transient/' + unit
            and active['ActiveState'] == 'active' and active['MainPID'] != '0', 'No owned active unit')
    save(case + '-active.json', active)
    verify_active(active, b1)
    save(case + '-verified.json', {'passed': True})
    return active


def verify_active(active, b1):
    now = capture()
    require(now['identity'] == b1['identity'] and now['features'] == b1['features'], 'Active NIC drift')
    xdp = now['link'].get('xdp', {})
    require(xdp.get('mode') == 2 and type(xdp.get('prog', {}).get('id')) is int, 'Explicit Generic not proven')
    pid = active['MainPID']
    require(hashlib.sha256(Path('/proc/' + pid + '/exe').read_bytes()).hexdigest()
            == json.loads((ROOT / 'scope.json').read_text())['binary_sha256'], 'Wrong running binary')
    for task in Path('/proc/' + pid + '/task').iterdir():
        st = dict(row.split(':', 1) for row in (task / 'status').read_text().splitlines() if ':' in row)
        require(all(int(v) != 0 for v in st['Uid'].split() + st['Gid'].split())
                and st['NoNewPrivs'].strip() == '1'
                and all(int(st[k], 16) == 0 for k in ('CapEff', 'CapPrm', 'CapInh', 'CapAmb')), 'Runtime privilege leak')
    fds = list(Path('/proc/' + pid + '/fdinfo').iterdir())
    require(len(fds) <= 64, 'FD bound')
    owned = False
    for fd in fds:
        try:
            text = fd.read_text()
        except FileNotFoundError:
            continue
        fields = dict(row.split(':', 1) for row in text.splitlines() if ':' in row)
        owned |= (fields.get('link_type', '').strip() == '6'
                  and fields.get('prog_id', '').strip() == str(xdp['prog']['id'])
                  and fields.get('ifindex', '').strip() == str(now['identity']['ifindex']))
    require(owned, 'FD-owned XDP link not proven')


def stop(manifest, case):
    context(manifest, cleanup=True)
    attempt, unit = read(case + '-attempt.json'), unit_for(case)
    require(attempt['unit'] == unit, 'Wrong owned unit')
    current = show(unit)
    require(current['Transient'] == 'yes' and current['FragmentPath'] == '/run/systemd/transient/' + unit,
            'Unknown unit preserved')
    if current['MainPID'] != '0':
        expected_argv = attempt['command'][attempt['command'].index(str(ROOT / 'veil-local-ban')):]
        argv = Path('/proc/' + current['MainPID'] + '/cmdline').read_bytes().rstrip(b'\0').split(b'\0')
        require(argv == [a.encode() for a in expected_argv], 'Unknown process command preserved')
    if case == 'kill' and current['ActiveState'] == 'active' and (ROOT / (case + '-active.json')).exists():
        expected = read(case + '-active.json')
        require(current['MainPID'] == expected['MainPID'] and current['ExecMainStartTimestampMonotonic'] == expected['ExecMainStartTimestampMonotonic'], 'PID ownership changed')
        run(['systemctl', 'kill', '--signal=SIGKILL', '--kill-whom=main', unit])
        end = time.monotonic() + 10
        while show(unit)['MainPID'] != '0' and time.monotonic() < end:
            time.sleep(.05)
    else:
        run(['systemctl', 'stop', unit])
    after, link = show(unit), capture()
    vacant(link['link'])
    require(after['MainPID'] == '0' and after['ActiveState'] in ('inactive', 'failed'), 'Unit not stopped')
    b1 = read('b1.json')
    require(link['identity'] == b1['identity'] and link['features'] == b1['features'], 'Stopped NIC drift')
    save(case + '-stopped.json', {'unit': after, 'snapshot': link})
    return {'stopped': True, 'case': case}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['init', 'prepare', 'start-normal', 'stop-normal', 'start-kill', 'stop-kill', 'restore'])
    parser.add_argument('manifest_sha256')
    parser.add_argument('--vm-id')
    parser.add_argument('--t0', type=float)
    args = parser.parse_args()
    if args.operation == 'init':
        require(os.geteuid() == 0, 'Root required')
        access.check_files(ROOT, args.manifest_sha256)
        m = metadata()
        require(args.vm_id == m['id'] and args.t0 is not None and 0 <= time.time() - args.t0 < 720,
                'Expected create-response ID and fresh T0 required')
        b0 = capture()
        vacant(b0['link'])
        require(capture() == b0, 'Unstable B0; no SET')
        save('session.json', {'metadata': m, 'boot': b0['identity']['boot'], 't0': args.t0})
        save('b0.json', b0)
        result = {'initialized': True, 'id': m['id']}
    elif args.operation == 'prepare':
        result = prepare(args.manifest_sha256)
    elif args.operation == 'restore':
        result = restore(args.manifest_sha256)
    else:
        action, case = args.operation.split('-')
        result = start(args.manifest_sha256, case) if action == 'start' else stop(args.manifest_sha256, case)
    print(json.dumps(result, indent=2))
