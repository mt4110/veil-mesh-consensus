#!/usr/bin/env python3
"""Bounded feature GET and pure change plans. This module never executes SET."""
import array
import ctypes
import fcntl
import json
import os
from pathlib import Path
import socket
import struct
import sys
import time

TARGETS = ('rx-lro', 'rx-gro-hw')
ALIASES = {'rx-lro': 'lro', 'rx-gro-hw': 'rx-gro-hw'}
MAX_FEATURES = 256


def require(ok, message):
    if not ok:
        raise ValueError(message)


def decode(names, blocks, count):
    require(type(count) is int and 1 <= count <= MAX_FEATURES, 'Feature count outside bound')
    require(len(names) == count * 32 and len(blocks) == ((count + 31) // 32) * 4,
            'Incomplete feature reply')
    result = []
    seen = set()
    for bit in range(count):
        raw = names[bit * 32:(bit + 1) * 32]
        require(b'\0' in raw, 'Unterminated feature name')
        name, padding = raw.split(b'\0', 1)
        require(not any(padding), 'Malformed feature name padding')
        name = name.decode('ascii')
        require(not name or name not in seen, 'Duplicate feature name')
        seen.add(name)
        row = {'index': bit, 'name': name}
        for i, key in enumerate(('available', 'wanted', 'active', 'never_changed')):
            row[key] = bool(blocks[(bit // 32) * 4 + i] & (1 << (bit % 32)))
        result.append(row)
    validate(result)
    return result


def validate(rows):
    require(type(rows) is list and 1 <= len(rows) <= MAX_FEATURES, 'Invalid feature list')
    seen = set()
    for i, r in enumerate(rows):
        require(type(r) is dict and set(r) == {'index', 'name', 'available', 'wanted', 'active', 'never_changed'},
                'Unknown feature schema')
        require(type(r['index']) is int and r['index'] == i and type(r['name']) is str
                and len(r['name']) < 32 and r['name'].isascii()
                and all(32 < ord(c) < 127 for c in r['name']), 'Invalid feature name/index')
        require(not r['name'] or r['name'] not in seen, 'Duplicate feature name')
        seen.add(r['name'])
        require(all(type(r[k]) is bool for k in ('available', 'wanted', 'active', 'never_changed')),
                'Feature booleans required')
    require(all(name in seen for name in TARGETS), 'Missing active/wanted target feature')


def get_features(iface):
    require(sys.platform == 'linux' and 0 < len(iface.encode()) < 16 and '\0' not in iface,
            'Invalid Linux interface')
    require(ctypes.sizeof(ctypes.c_void_p) == 8, '64-bit ifreq ABI required')
    def get(values):
        require(values[0] in (0x37, 0x1b, 0x3a), 'GET commands only')
        buf = array.array('I', values)
        require(buf.itemsize == 4, 'u32 ABI required')
        request = bytearray(struct.pack('16sP', iface.encode(), buf.buffer_info()[0]) + bytes(16))
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM | socket.SOCK_CLOEXEC) as fd:
            fcntl.ioctl(fd, 0x8946, request, True)
        return list(buf)
    mask = struct.pack('=Q', 1 << 4)
    halves = struct.unpack('=II', mask)
    info = get([0x37, 0, *halves, 0, 0])
    require(info[:4] == [0x37, 0, *halves] and 1 <= info[4] <= MAX_FEATURES,
            'Feature string set unavailable')
    count = info[4]
    strings = get([0x1b, 4, count] + [0] * (count * 8))
    require(strings[:3] == [0x1b, 4, count], 'Feature names changed')
    words = (count + 31) // 32
    blocks = get([0x3a, words] + [0] * (4 * words))
    require(blocks[:2] == [0x3a, words], 'Feature count changed')
    return decode(struct.pack('=' + 'I' * (len(strings) - 3), *strings[3:]), blocks[2:], count)


def admit_off(rows):
    validate(rows)
    require(all(not r['active'] and not r['wanted'] for r in rows if r['name'] in TARGETS),
            'LRO/hardware GRO must be off in active AND wanted')


def same_layout(before, current):
    validate(before)
    validate(current)
    require(len(before) == len(current), 'Feature layout changed')
    for old, new in zip(before, current):
        require(all(old[k] == new[k] for k in ('index', 'name', 'available', 'never_changed')),
                'Feature identity/capability changed')


def off_args(before, current):
    same_layout(before, current)
    require(before == current, 'B0 changed; do not configure')
    args = []
    for r in before:
        if r['name'] in TARGETS and (r['wanted'] or r['active']):
            require(r['available'] and not r['never_changed'], 'Required feature is fixed or unknown')
            args.extend([ALIASES[r['name']], 'off'])
    return args


def off_result(before, current):
    same_layout(before, current)
    admit_off(current)
    require(all(a == b for a, b in zip(before, current) if a['name'] not in TARGETS),
            'Unrelated feature changed; do not claim preservation')


def restore_args(before, current):
    same_layout(before, current)
    require(all(a == b for a, b in zip(before, current) if a['name'] not in TARGETS),
            'Unrelated change; refuse restoration')
    args = []
    for old, new in zip(before, current):
        if old['name'] not in TARGETS:
            continue
        require(not (new['wanted'] and not old['wanted']) and not (new['active'] and not old['active']),
                'Unknown target change; refuse restoration')
        if old['wanted'] != new['wanted']:
            require(old['available'] and not old['never_changed'], 'Cannot restore fixed feature')
            args.extend([ALIASES[old['name']], 'on' if old['wanted'] else 'off'])
    return args


def set_command(args):
    require(type(args) is list and len(args) in (0, 2, 4), 'Invalid feature change size')
    seen = set()
    for name, value in zip(args[::2], args[1::2]):
        require(name in ALIASES.values() and name not in seen and value in ('on', 'off'),
                'Only two fixed feature names/boolean values allowed')
        seen.add(name)
    return ['/usr/sbin/ethtool', '-K', 'ens4', *args] if args else None


def route_get_smoke():
    """Read-only loopback transport gate; no BPF load or feature SET."""
    require(sys.platform == 'linux', 'Linux required')
    index = socket.if_nametoindex('lo')
    checked = []
    for kind in (18, 38, 46):  # GETLINK, GETQDISC, GETTFILTER
        dump = kind != 18
        body = bytearray(20 if dump else 16)
        struct.pack_into('=I', body, 4, index)
        request = struct.pack('=IHHII', 16 + len(body), kind, 0x301 if dump else 1, 1, 0) + body
        with socket.socket(socket.AF_NETLINK, socket.SOCK_DGRAM | socket.SOCK_CLOEXEC, socket.NETLINK_ROUTE) as fd:
            fd.bind((0, 0))
            fd.settimeout(1)
            fd.sendto(request, (0, 0))
            done = False
            deadline = time.monotonic() + 2
            for _ in range(16):
                require(time.monotonic() < deadline, 'Netlink GET deadline')
                data, _, flags, sender = fd.recvmsg(65536)
                require(sender[0] == 0 and not flags & socket.MSG_TRUNC, 'Truncated/non-kernel netlink')
                while data:
                    require(len(data) >= 16, 'Truncated netlink header')
                    size, ty, fl, seq, _ = struct.unpack_from('=IHHII', data)
                    require(16 <= size <= len(data) and seq == 1 and not fl & 0x10, 'Invalid netlink reply')
                    if ty == 2:
                        raise ValueError('Kernel GET error')
                    if ty == 3:
                        require(dump and (size == 16 or (size >= 20 and struct.unpack_from('=i', data, 16)[0] == 0)), 'Failed dump')
                        done = True
                    else:
                        require(ty == {18: 16, 38: 36, 46: 44}[kind], 'Unexpected netlink reply')
                        done = not dump
                    data = data[(size + 3) & ~3:]
                if done:
                    break
            require(done, 'Netlink response bound')
        checked.append(kind)
    return {'read_only_loopback_gets': checked, 'passed': True}


if __name__ == '__main__':
    if sys.argv[1:] == ['route-get-smoke']:
        result = route_get_smoke()
    else:
        require(len(sys.argv) == 3 and sys.argv[1] == 'get', 'Usage: features.py get IFACE | route-get-smoke (read-only)')
        result = {'features': get_features(sys.argv[2])}
    print(json.dumps(result, indent=2))
