#!/usr/bin/env python3
"""Package the fixed trial assets. No cloud, SSH, chmod or deletion operations."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import tarfile

REPO = Path(__file__).resolve().parents[1]
MAX_BYTES = 64 * 1024 * 1024
MODES = {'veil-local-ban': 0o755, 'veil_xdp_kern.o': 0o644,
         'driver.py': 0o644, 'profile.service': 0o644,
         'nic-check.py': 0o644, 'access-check.py': 0o644, 'manifest.json': 0o644}
SOURCES = {
    'veil-local-ban': ('artifacts/gcp-a-prep-9olhrkec/veil-local-ban', 'e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890'),
    'veil_xdp_kern.o': ('artifacts/gcp-a-prep-9olhrkec/veil_xdp_kern.o', '2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f'),
    'driver.py': ('artifacts/gcp-a-prep-9olhrkec/driver.py', 'fcf851556d39f399eaab2f09ae6187eac55561b3e69a882a251ebfbb9c51de00'),
    'profile.service': ('artifacts/gcp-xdp-live-b6psh2dz/profile-trial.service', '3305fec42f73ffe4282443fc2ed9dc844ab42671e83f736f20c8729c87eba130'),
    'nic-check.py': ('artifacts/gcp-xdp-live-b6psh2dz/nic-check.py', 'f7bc5c6bbed48845db30d5b49f7f0b34911404eb737cfcd5f54e7b7714a7e024'),
}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def stable(s):
    return tuple(getattr(s, k) for k in ('st_dev', 'st_ino', 'st_mode', 'st_uid',
                 'st_gid', 'st_nlink', 'st_size', 'st_mtime_ns', 'st_ctime_ns'))


def read_source(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as f:
        before = os.fstat(f.fileno())
        require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1
                and 0 < before.st_size <= MAX_BYTES, 'Unsafe source file')
        data = f.read(MAX_BYTES + 1)
        after = os.fstat(f.fileno())
    require(stable(before) == stable(after) and len(data) == before.st_size, 'Source changed while reading')
    return data


def check_member(member):
    require(member.name in MODES and member.isfile(), 'Unknown path or nonregular member')
    require(member.uid == member.gid == 0 and member.mode == MODES[member.name],
            'Unsafe owner/mode: ' + member.name)
    require(not member.pax_headers and 0 < member.size <= MAX_BYTES,
            'Unexpected attributes or size')


def verify(archive):
    data = {}
    with tarfile.open(archive, 'r:gz') as t:
        for member in t:
            check_member(member)
            require(member.name not in data, 'Duplicate member')
            require(sum(len(x) for x in data.values()) + member.size <= MAX_BYTES,
                    'Package too large')
            data[member.name] = t.extractfile(member).read()
    require(set(data) == set(MODES), 'Incomplete package')
    require(len(data['manifest.json']) <= 8192, 'Manifest too large')
    manifest = json.loads(data['manifest.json'])
    require(manifest.get('version') == 1 and set(manifest.get('files', {})) == set(MODES) - {'manifest.json'},
            'Unexpected manifest schema')
    for name, item in manifest['files'].items():
        require(item == {'sha256': digest(data[name]), 'bytes': len(data[name]), 'mode': MODES[name]},
                'Manifest mismatch: ' + name)
        if name in SOURCES:
            require(item['sha256'] == SOURCES[name][1], 'Wrong trial asset: ' + name)
    return {'archive_sha256': digest(Path(archive).read_bytes()),
            'manifest_sha256': digest(data['manifest.json']), 'files': manifest['files']}


def build(output):
    require(not Path(output).exists(), 'Preserve existing package')
    files = {}
    for name, (relative, expected) in SOURCES.items():
        files[name] = read_source(REPO / relative)
        require(digest(files[name]) == expected, 'Source hash mismatch: ' + name)
    files['access-check.py'] = read_source(Path(__file__).with_name('local-ban-trial-access.py'))
    manifest = {'version': 1, 'files': {name: {'sha256': digest(data), 'bytes': len(data), 'mode': MODES[name]}
                                      for name, data in files.items()}}
    files['manifest.json'] = (json.dumps(manifest, sort_keys=True, indent=2) + '\n').encode()
    # Fresh USTAR entries exclude source ACL/xattr/PAX and source uid/gid/mode.
    with open(output, 'xb') as f, tarfile.open(fileobj=f, mode='w:gz', format=tarfile.USTAR_FORMAT) as t:
        for name, data in files.items():
            member = tarfile.TarInfo(name)
            member.uid = member.gid = 0
            member.uname = member.gname = 'root'
            member.mode = MODES[name]
            member.size = len(data)
            member.mtime = 0
            t.addfile(member, io.BytesIO(data))
    return verify(output)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['build', 'verify'])
    parser.add_argument('archive', type=Path)
    args = parser.parse_args()
    print(json.dumps(build(args.archive) if args.operation == 'build' else verify(args.archive), indent=2))
