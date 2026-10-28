#!/usr/bin/env python3
"""Read-only guest gate. Never starts serve, opens network or changes ownership."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import pwd
import stat
import subprocess
import sys

MAX_BYTES = 64 * 1024 * 1024
MODES = {'veil-local-ban': 0o755, 'veil_xdp_kern.o': 0o644, 'driver.py': 0o644,
         'profile.service': 0o644, 'nic-check.py': 0o644, 'access-check.py': 0o644}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def stable(s):
    return tuple(getattr(s, k) for k in ('st_dev', 'st_ino', 'st_mode', 'st_uid',
                 'st_gid', 'st_nlink', 'st_size', 'st_mtime_ns', 'st_ctime_ns'))


def checked_file(path, expected_mode):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as f:
        before = os.fstat(f.fileno())
        require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1 and before.st_uid == 0
                and before.st_gid == 0 and stat.S_IMODE(before.st_mode) == expected_mode,
                'Unexpected root owner/mode/link: ' + path.name)
        require(0 < before.st_size <= MAX_BYTES, 'Unexpected file size')
        data = f.read(MAX_BYTES + 1)
        require(stable(before) == stable(os.fstat(f.fileno())) and len(data) == before.st_size,
                'File changed while reading')
    return data


def check_files(root, expected_manifest):
    require(root.is_absolute() and root.parent == Path('/run') and root.name == 'veil-gcp-xdp-20261027',
            'Unexpected trial root')
    for path in (Path('/'), Path('/run'), root):
        s = path.lstat()
        require(stat.S_ISDIR(s.st_mode) and s.st_uid == 0 and not s.st_mode & 0o022,
                'Unsafe parent: ' + str(path))
    data = checked_file(root / 'manifest.json', 0o644)
    require(len(data) <= 8192 and hashlib.sha256(data).hexdigest() == expected_manifest,
            'Manifest digest mismatch')
    manifest = json.loads(data)
    require(manifest.get('version') == 1 and set(manifest.get('files', {})) == set(MODES),
            'Unexpected manifest schema')
    for name, mode in MODES.items():
        body = checked_file(root / name, mode)
        require(manifest['files'][name] == {'sha256': hashlib.sha256(body).hexdigest(),
                                          'bytes': len(body), 'mode': mode}, 'Asset mismatch: ' + name)
    return manifest


def access_only(root, expected_manifest):
    """Real nonroot DAC/read checks; CLI additionally requires native --help."""
    require(sys.platform == 'linux' and os.geteuid() != 0 and os.getegid() != 0,
            'A nonroot Linux process is required')
    status = dict(line.split(':', 1) for line in Path('/proc/self/status').read_text().splitlines() if ':' in line)
    require(status['NoNewPrivs'].strip() == '1'
            and all(int(status[k], 16) == 0 for k in ('CapEff', 'CapPrm', 'CapInh', 'CapAmb', 'CapBnd')),
            'Require NNP and zero capabilities')
    require(0 not in os.getgroups(), 'Root supplementary group is forbidden')
    manifest = check_files(root, expected_manifest)
    for path in (root, root / 'manifest.json', *(root / name for name in MODES)):
        require(not os.access(path, os.W_OK, effective_ids=True), 'Service can write package: ' + path.name)
    require(os.access(root / 'veil-local-ban', os.X_OK, effective_ids=True), 'Binary not executable')
    return {'uid': os.geteuid(), 'gid': os.getegid(), 'groups': os.getgroups(),
            'no_new_privs': True, 'capabilities_zero': True,
            'files_read_and_hashed': len(manifest['files']), 'package_writable': False,
            'binary_execute_access': True}


def user_gate(root, expected_manifest):
    result = access_only(root, expected_manifest)
    # Clap help exits before any serve/control/BPF path. Failure is never bypassed.
    p = subprocess.run([str(root / 'veil-local-ban'), '--help'], capture_output=True, text=True, timeout=3)
    require(p.returncode == 0 and 'Usage:' in p.stdout and 'serve' in p.stdout,
            'Actual service-user binary --help failed')
    return {**result, 'native_help_exit': p.returncode}


def service_gate(root, expected_manifest):
    """Mandatory future guest gate; no BPF capability or serve invocation."""
    require(sys.platform == 'linux' and os.geteuid() == 0, 'Linux root check required')
    require(os.uname().machine == 'x86_64', 'This trial requires x86_64')
    check_files(root, expected_manifest)
    unit = root.name + '-access.service'
    state = subprocess.run(['systemctl', 'show', unit, '-p', 'LoadState'],
                           capture_output=True, text=True, timeout=5)
    require(state.stdout.strip() == 'LoadState=not-found', 'Preserve existing access unit')
    try:
        pwd.getpwnam(root.name)
    except KeyError:
        pass
    else:
        raise ValueError('Preserve existing service account')
    properties = {'DynamicUser': 'yes', 'User': root.name, 'Group': root.name,
                  'NoNewPrivileges': 'yes', 'CapabilityBoundingSet': '', 'AmbientCapabilities': '',
                  'PrivateNetwork': 'yes', 'PrivateTmp': 'yes', 'PrivateDevices': 'yes',
                  'ProtectSystem': 'strict', 'ProtectHome': 'yes',
                  'ReadOnlyPaths': str(root), 'RestrictAddressFamilies': 'AF_UNIX',
                  'Restart': 'no', 'RuntimeMaxSec': '15s', 'TimeoutStopSec': '10s',
                  'KillMode': 'control-group', 'UMask': '0077'}
    command = ['systemd-run', '--unit=' + unit, '--service-type=exec', '--wait', '--pipe', '--quiet']
    command += ['--property=' + key + '=' + value for key, value in properties.items()]
    command += ['/usr/bin/python3', '-I', '-B', str(root / 'access-check.py'),
                'user-check', str(root), expected_manifest]
    p = subprocess.run(command, capture_output=True, text=True, timeout=30)
    require(p.returncode == 0, 'Nonroot service gate failed: ' + p.stderr[:1000])
    result = json.loads(p.stdout)
    require(result.get('native_help_exit') == 0 and result.get('capabilities_zero') is True
            and result.get('package_writable') is False, 'Incomplete nonroot gate result')
    return {'root_owner_modes_hashes': True, 'unit': unit, 'nonroot': result}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['root-check', 'user-check', 'service-check'])
    parser.add_argument('root', type=Path)
    parser.add_argument('manifest_sha256')
    args = parser.parse_args()
    if args.operation == 'root-check':
        require(sys.platform == 'linux' and os.geteuid() == 0, 'Linux root check required')
        check_files(args.root, args.manifest_sha256)
        result = {'root_owner_modes_hashes': True}
    elif args.operation == 'service-check':
        result = service_gate(args.root, args.manifest_sha256)
    else:
        result = user_gate(args.root, args.manifest_sha256)
    print(json.dumps(result, indent=2))
