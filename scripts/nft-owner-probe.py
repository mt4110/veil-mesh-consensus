#!/usr/bin/env python3
"""Prepared Linux owner-table probe. Default: describe only. NOT a BAN adapter.

No nft executable, delete/flush, chain/rule/set, setns, sudo, SSH or cloud calls.
Live mode requires the separately approved private systemd test boundary.
UAPI reference: Linux v6.8 include/uapi/linux/{netlink.h,netfilter/nf_tables.h,
netfilter/nfnetlink.h}. No kernel implementation code is copied.
"""
import argparse
import datetime
import errno
import json
import os
from pathlib import Path
import re
import select
import signal
import socket
import stat
import struct
import subprocess
import sys
import time

UNIT = 'veil-nft-owner-probe-20261027a.service'
RUNTIME = Path('/run/veil-nft-owner-probe-20261027a')
EVIDENCE = RUNTIME / 'evidence'
HOST_REFERENCE = RUNTIME / 'host-reference.json'
MAX_REFERENCE_BYTES = 4096
GUARD = 'vno_guard'
OWNED = 'vno_owned'
TABLE_NAMES = (GUARD, OWNED)
NET_ADMIN = 1 << 12
NFT = 10 << 8
NFT_NEW_TABLE = NFT
NFT_GET_TABLE = NFT | 1
BATCH_BEGIN, BATCH_END = 16, 17
REQUEST, ACK, DUMP, CREATE_EXCL = 1, 4, 0x300, 0x600
MULTI, DUMP_INTR = 2, 0x10
NL_ERROR, NL_DONE, NL_OVERRUN = 2, 3, 4
OWNER = 2
MAX_BYTES = 65_536
MAX_MESSAGES = 64
DEADLINE = 2.0
HEADER = struct.Struct('=IHHII')
ATTR = struct.Struct('=HH')


class Refusal(RuntimeError):
    pass


def require(ok, message):
    if not ok:
        raise Refusal(message)


def align4(size):
    return (size + 3) & ~3


def attribute(kind, value):
    size = ATTR.size + len(value)
    return ATTR.pack(size, kind) + value + bytes(align4(size) - size)


def message(kind, flags, sequence, payload):
    require(0 < sequence < 2**32, 'Invalid sequence')
    size = HEADER.size + len(payload)
    return HEADER.pack(size, kind, flags, sequence, 0) + payload + bytes(align4(size) - size)


def create_owner_batch(name, sequence):
    require(name in TABLE_NAMES, 'Only fixed probe table names are allowed')
    require(sequence <= 2**32 - 3, 'Sequence exhausted')
    generation = struct.pack('!BBH', 0, 0, 10)
    table = (struct.pack('!BBH', 1, 0, 0) + attribute(1, name.encode('ascii') + b'\0')
             + attribute(2, struct.pack('!I', OWNER)))
    return (message(BATCH_BEGIN, REQUEST, sequence, generation)
            + message(NFT_NEW_TABLE, REQUEST | ACK | CREATE_EXCL, sequence + 1, table)
            + message(BATCH_END, REQUEST, sequence + 2, generation))


def parse_messages(data, sender=(0, 0), receive_flags=0):
    require(sender == (0, 0), 'Reply not from kernel unicast')
    require(not receive_flags & (getattr(socket, 'MSG_TRUNC', 0x20) | getattr(socket, 'MSG_CTRUNC', 8)),
            'Truncated reply')
    require(0 < len(data) <= MAX_BYTES, 'Invalid datagram size')
    offset = 0
    rows = []
    while offset < len(data):
        require(len(rows) < MAX_MESSAGES and len(data) - offset >= HEADER.size, 'Incomplete header or message limit')
        length, kind, flags, seq, pid = HEADER.unpack_from(data, offset)
        require(length >= HEADER.size and offset + align4(length) <= len(data), 'Invalid message boundary')
        require(not flags & DUMP_INTR and kind != NL_OVERRUN, 'Incomplete dump or overrun')
        rows.append((kind, flags, seq, pid, data[offset + HEADER.size:offset + length]))
        offset += align4(length)
    return rows


def parse_table(payload):
    require(4 <= len(payload) <= MAX_BYTES, 'Invalid nfgenmsg size')
    family, version, resource = struct.unpack_from('!BBH', payload)
    # Replies carry the ruleset generation's low 16 bits in res_id (not 0).
    require(version == 0, 'Unexpected nfgenmsg version')
    attrs = {}
    offset = 4
    while offset < len(payload):
        require(offset + ATTR.size <= len(payload), 'Short attribute')
        length, flags_kind = ATTR.unpack_from(payload, offset)
        require(length >= ATTR.size and offset + align4(length) <= len(payload), 'Invalid attribute length')
        kind = flags_kind & 0x3fff
        require(kind not in attrs or kind == 5, 'Duplicate attribute')
        attrs[kind] = payload[offset + ATTR.size:offset + length]
        offset += align4(length)
    name = attrs.get(1, b'')
    require(1 < len(name) <= 256 and name[-1:] == b'\0' and b'\0' not in name[:-1], 'Invalid table name')
    try:
        name = name[:-1].decode('ascii')
    except UnicodeDecodeError as error:
        raise Refusal('Unknown non-ASCII table: preserve and refuse') from error
    result = {'family': family, 'name': name, 'generation': resource}
    for kind, key in [(2, 'flags'), (3, 'use'), (7, 'owner')]:
        if kind in attrs:
            require(len(attrs[kind]) == 4, 'Invalid u32 attribute')
            result[key] = struct.unpack('!I', attrs[kind])[0]
    return result


class Netfilter:
    def __init__(self):
        self.sock = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 12)
        self.sock.set_inheritable(False)
        self.sock.bind((0, 0))
        self.port = self.sock.getsockname()[0]
        require(self.port > 0, 'Unassigned Netlink port')
        self.sequence = 100

    def close(self):
        self.sock.close()

    def receive(self, sequence, dump, request_kind):
        end = time.monotonic() + DEADLINE
        total = 0
        messages = 0
        tables = []
        while True:
            remaining = end - time.monotonic()
            require(remaining > 0, 'Reply timeout: outcome unknown')
            self.sock.settimeout(remaining)
            data, _, flags, sender = self.sock.recvmsg(MAX_BYTES)
            total += len(data)
            require(total <= MAX_BYTES, 'Reply byte budget exceeded')
            for kind, msgflags, seq, _, payload in parse_messages(data, sender, flags):
                messages += 1
                require(messages <= MAX_MESSAGES and seq == sequence, 'Reply limit or unexpected sequence')
                if kind == NL_ERROR:
                    require(len(payload) >= 20, 'Short ACK')
                    code = struct.unpack_from('=i', payload)[0]
                    _, original_kind, _, original_seq, _ = HEADER.unpack_from(payload, 4)
                    require(original_kind == request_kind and original_seq == sequence, 'ACK binding mismatch')
                    require(code <= 0, 'Invalid ACK error')
                    if code:
                        raise Refusal('Kernel rejected request: ' + errno.errorcode.get(-code, str(code)))
                    require(not dump, 'Unexpected ACK during table dump')
                    return None
                if kind == NL_DONE:
                    require(dump and len(payload) >= 4 and struct.unpack_from('=i', payload)[0] == 0,
                            'Failed or unexpected dump terminator')
                    require(len({row['generation'] for row in tables}) <= 1, 'Generation changed during dump')
                    return tables
                require(dump and kind == NFT_NEW_TABLE and msgflags & MULTI, 'Unexpected reply type')
                tables.append(parse_table(payload))

    def create(self, name):
        sequence = self.sequence
        self.sequence += 3
        data = create_owner_batch(name, sequence)
        require(self.sock.sendto(data, (0, 0)) == len(data), 'Short send: outcome unknown')
        self.receive(sequence + 1, False, NFT_NEW_TABLE)

    def tables(self):
        sequence = self.sequence
        self.sequence += 1
        # AF_UNSPEC: do not mistake other-family tables for an empty namespace.
        request = message(NFT_GET_TABLE, REQUEST | DUMP, sequence, struct.pack('!BBH', 0, 0, 0))
        require(self.sock.sendto(request, (0, 0)) == len(request), 'Short dump request')
        return self.receive(sequence, True, NFT_GET_TABLE)


def validate_boundary(facts):
    require(facts['platform'] == 'linux', 'Linux-only live probe')
    require(facts['uid'] == 0, 'Root test fixture required; not production privilege proof')
    require(facts['netns'] != facts['init_netns'], 'Refuse host network namespace')
    require(facts['pidns'] == facts['init_pidns'] and facts['userns'] == facts['init_userns'],
            'Require host PID/user namespace for trustworthy isolation checks')
    require(facts['cgroup'] == '/system.slice/' + UNIT, 'Require reviewed transient test unit')
    require(facts['cap_eff'] == NET_ADMIN and facts['cap_prm'] == NET_ADMIN and facts['cap_bnd'] == NET_ADMIN,
            'Require CAP_NET_ADMIN only')
    require(facts['no_new_privs'] == 1, 'Require NoNewPrivileges')
    require(facts['nf_tables_loaded'], 'Require pre-existing nf_tables; do not trigger a module setup workflow')
    require(facts['interfaces'] == ['lo'], 'Require a fresh disconnected network namespace')


def namespace_id(path):
    s = os.stat(path)
    return [s.st_dev, s.st_ino]


def parse_host_reference(raw, boot_id):
    require(0 < len(raw) <= MAX_REFERENCE_BYTES, 'Invalid host reference size')

    def unique_object(pairs):
        obj = {}
        for key, value in pairs:
            require(key not in obj, 'Duplicate host reference field')
            obj[key] = value
        return obj

    try:
        reference = json.loads(raw.decode('utf-8'), object_pairs_hook=unique_object)
    except (UnicodeError, ValueError, RecursionError) as error:
        raise Refusal('Invalid host reference JSON') from error
    keys = {'version', 'boot_id', 'init_netns', 'init_pidns', 'init_userns'}
    require(type(reference) is dict and set(reference) == keys, 'Unexpected host reference fields')
    require(type(reference['version']) is int and reference['version'] == 1,
            'Unsupported host reference version')
    expected_boot = reference['boot_id']
    require(type(expected_boot) is str and re.fullmatch(
        r'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}', expected_boot),
        'Invalid reference boot ID')
    require(expected_boot == boot_id, 'Host reference belongs to another boot')
    for key in ['init_netns', 'init_pidns', 'init_userns']:
        pair = reference[key]
        require(type(pair) is list and len(pair) == 2
                and all(type(value) is int and 0 < value < 2**64 for value in pair),
                'Invalid namespace identity in host reference')
    return reference


def load_host_reference(boot_id):
    # Open the fixed root-only directory first, then resolve only a fixed basename.
    # No following symlinks, file creation, ownership changes or permission repairs.
    directory_fd = file_fd = None
    try:
        flags = os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW
        directory_fd = os.open(RUNTIME, flags | os.O_DIRECTORY)
        directory = os.fstat(directory_fd)
        require(stat.S_ISDIR(directory.st_mode) and directory.st_uid == 0
                and stat.S_IMODE(directory.st_mode) == 0o700, 'Untrusted runtime directory')
        file_fd = os.open(HOST_REFERENCE.name, flags | os.O_NONBLOCK, dir_fd=directory_fd)
        before = os.fstat(file_fd)
        require(stat.S_ISREG(before.st_mode) and before.st_uid == 0
                and stat.S_IMODE(before.st_mode) == 0o400 and before.st_nlink == 1,
                'Untrusted host reference owner/type/mode/links')
        require(0 < before.st_size <= MAX_REFERENCE_BYTES, 'Invalid host reference file size')
        raw = b''
        while len(raw) <= MAX_REFERENCE_BYTES:
            block = os.read(file_fd, MAX_REFERENCE_BYTES + 1 - len(raw))
            if not block:
                break
            raw += block
        after = os.fstat(file_fd)
        fields = ['st_dev', 'st_ino', 'st_mode', 'st_uid', 'st_gid', 'st_nlink', 'st_size',
                  'st_mtime_ns', 'st_ctime_ns']
        require(all(getattr(before, key) == getattr(after, key) for key in fields)
                and len(raw) == before.st_size, 'Host reference changed during read')
        return parse_host_reference(raw, boot_id)
    except OSError as error:
        raise Refusal('Cannot read fixed host reference; errno=' + str(error.errno)) from error
    finally:
        if file_fd is not None:
            os.close(file_fd)
        if directory_fd is not None:
            os.close(directory_fd)


def inspect_boundary():
    require(sys.platform == 'linux', 'Linux-only live probe')
    values = dict(line.split(':', 1) for line in Path('/proc/self/status').read_text().splitlines() if ':' in line)
    cgroups = Path('/proc/self/cgroup').read_text().splitlines()
    require(len(cgroups) == 1 and cgroups[0].startswith('0::'), 'Require unified cgroup v2')
    require(os.geteuid() == 0, 'Root test fixture required')
    boot_id = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    reference = load_host_reference(boot_id)
    # Check namespace identities *before* running ip or creating any Netlink socket.
    facts = {'platform': sys.platform, 'uid': os.geteuid(),
             'netns': namespace_id('/proc/self/ns/net'), 'init_netns': reference['init_netns'],
             'pidns': namespace_id('/proc/self/ns/pid'), 'init_pidns': reference['init_pidns'],
             'userns': namespace_id('/proc/self/ns/user'), 'init_userns': reference['init_userns'],
             'boot_id': boot_id,
             'cgroup': cgroups[0][3:], 'cap_eff': int(values['CapEff'], 16),
             'cap_prm': int(values['CapPrm'], 16), 'cap_bnd': int(values['CapBnd'], 16),
             'no_new_privs': int(values['NoNewPrivs']), 'interfaces': ['lo'],
             'nf_tables_loaded': Path('/sys/module/nf_tables').is_dir()}
    validate_boundary(facts)
    p = subprocess.run(['/usr/sbin/ip', '-j', 'link', 'show'], stdin=subprocess.DEVNULL,
                       capture_output=True, text=True, timeout=DEADLINE)
    require(p.returncode == 0 and len(p.stdout) <= MAX_BYTES, 'Cannot inspect private links')
    facts['interfaces'] = [row['ifname'] for row in json.loads(p.stdout)]
    validate_boundary(facts)
    return facts


def wait_child(pid, seconds=DEADLINE):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        seen, status = os.waitpid(pid, os.WNOHANG)
        if seen == pid:
            return status
        time.sleep(0.02)
    raise Refusal('Child not reaped within deadline')


def read_child(fd):
    end = time.monotonic() + 2 * DEADLINE
    data = b''
    while time.monotonic() < end:
        ready, _, _ = select.select([fd], [], [], max(0, end - time.monotonic()))
        require(ready, 'Child readiness timeout')
        block = os.read(fd, 512)
        require(block, 'Child exited before readiness')
        data += block
        require(len(data) <= 512, 'Child report limit')
        if b'\n' in data:
            require(data.endswith(b'\n') and data.count(b'\n') == 1, 'Malformed child report')
            return json.loads(data)
    raise Refusal('Child readiness timeout')


def require_child_alive(pid):
    # Observe without reaping: the failure path must retain ownership of this PID.
    require(os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is None,
            'Owner exited during socket-only observation')


def verify_table(row, name, port):
    observed = {key: value for key, value in row.items() if key != 'generation'}
    require(observed == {'family': 1, 'name': name, 'flags': OWNER, 'use': 0, 'owner': port},
            'Unexpected table owner/flags/use; preserve and stop')


def absent_with_guard(observer, guard, name):
    end = time.monotonic() + DEADLINE
    while time.monotonic() < end:
        rows = observer.tables()
        require(len(rows) <= 2, 'Unexpected table appeared')
        guards = [row for row in rows if row.get('name') == GUARD]
        require(len(guards) == 1, 'Control table lost')
        verify_table(guards[0], GUARD, guard.port)
        if len(rows) == 1:
            return rows
        require(any(row.get('name') == name for row in rows), 'Unknown table: preserve and stop')
        time.sleep(0.05)
    raise Refusal('Owner table persists after socket/process exit')


def run_case(observer, guard, kind):
    ready_r, ready_w = os.pipe()
    command_r, command_w = os.pipe()
    pid = os.fork()
    if pid == 0:
        peer = None
        try:
            os.close(ready_r); os.close(command_w)
            # Inherited control/observer references must not keep their sockets alive.
            observer.close(); guard.close()
            peer = Netfilter()  # owner socket is created only in this child
            require(not os.get_inheritable(peer.sock.fileno()), 'Owner FD inheritable')
            peer.create(OWNED)
            os.write(ready_w, (json.dumps({'owner_port': peer.port, 'pid': os.getpid()}) + '\n').encode())
            require(os.read(command_r, 1) == b'C', 'No close instruction')
            peer.close(); peer = None
            os.write(ready_w, (json.dumps({'closed': True, 'pid': os.getpid()}) + '\n').encode())
            # Remain alive while the independent observer checks socket cleanup.
            require(os.read(command_r, 1) == b'E', 'No exit instruction')
            os._exit(0)
        except BaseException as error:
            try:
                os.write(ready_w, (json.dumps({'error': str(error)[:200]}) + '\n').encode())
            except OSError:
                pass
            if peer is not None:
                peer.close()
            os._exit(1)
    os.close(ready_w); os.close(command_r)
    try:
        ready = read_child(ready_r)
        require(set(ready) == {'owner_port', 'pid'} and ready['pid'] == pid, 'Child initialization failed: ' + repr(ready))
        rows = observer.tables()
        require(len(rows) == 2, 'Expected exactly two empty tables')
        for row in rows:
            verify_table(row, row['name'], guard.port if row['name'] == GUARD else ready['owner_port'])
        require({row['name'] for row in rows} == set(TABLE_NAMES), 'Unknown table')
        if kind == 'close':
            os.write(command_w, b'C')
            require(read_child(ready_r) == {'closed': True, 'pid': pid}, 'Socket close unconfirmed')
            require_child_alive(pid)
            after = absent_with_guard(observer, guard, OWNED)
            require_child_alive(pid)
            os.write(command_w, b'E')
        else:
            require(kind == 'sigkill', 'Unknown case')
            os.kill(pid, signal.SIGKILL)
        status = wait_child(pid)
        reaped = pid; pid = None
        require((kind == 'close' and os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0)
                or (kind == 'sigkill' and os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGKILL),
                'Unexpected child termination')
        if kind == 'sigkill':
            after = absent_with_guard(observer, guard, OWNED)
        return {'case': kind, 'child': reaped, 'status': status, 'before': rows, 'after': after,
                'owner_table_absent': True, 'control_preserved': True,
                'observed_while_owner_alive': kind == 'close'}
    finally:
        os.close(ready_r); os.close(command_w)
        if pid is not None:
            # Still our unreaped child. Never signal a stored PID from another run.
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            wait_child(pid)


def unit_command():
    """Render only. This module never invokes systemd-run."""
    properties = [
        'Slice=system.slice', 'User=root', 'PrivateNetwork=yes', 'PrivateUsers=no',
        'CapabilityBoundingSet=CAP_NET_ADMIN', 'NoNewPrivileges=yes',
        'RestrictNamespaces=yes', 'RestrictAddressFamilies=AF_UNIX AF_NETLINK',
        'ProtectSystem=strict', 'ProtectHome=yes', 'PrivateTmp=yes', 'PrivateDevices=yes',
        'ProtectKernelTunables=yes', 'ProtectKernelModules=yes', 'ProtectKernelLogs=yes',
        'ProtectControlGroups=yes', 'RestrictSUIDSGID=yes', 'UMask=0077',
        'ReadWritePaths=' + str(EVIDENCE), 'WorkingDirectory=' + str(RUNTIME),
        'RuntimeMaxSec=60s', 'TimeoutStopSec=5s', 'KillMode=control-group', 'Restart=no',
        'TasksMax=8', 'MemoryMax=64M',
    ]
    return (['/usr/bin/systemd-run', '--unit=' + UNIT, '--service-type=exec', '--wait']
            + ['--property=' + prop for prop in properties]
            + ['/usr/bin/python3', '-I', '-B', str(RUNTIME / 'nft-owner-probe.py'),
               '--execute-isolated-owner-probe'])


def plan():
    return {'mode': 'prepared-owner-probe', 'live_executed': False, 'ban_apply_supported': False,
            'unit': UNIT, 'runtime': str(RUNTIME), 'max_tables': 2,
            'host_reference': {'path': str(HOST_REFERENCE), 'owner_uid': 0, 'mode': '0400',
                               'version': 1, 'max_bytes': MAX_REFERENCE_BYTES,
                               'same_boot_required': True, 'pid_1_access_in_probe': False},
            'table_flags': ['owner'], 'chain_rule_set_count': 0,
            'cases': ['owner-socket-close', 'owner-process-SIGKILL'],
            'host_configuration_change': False, 'private_kernel_mutation_if_approved': True,
            'ttl_contract': 'unchanged-absolute-BOOTTIME; no TTL probe',
            'review_only_systemd_run_argv': unit_command(),
            'requires': 'separate approval and reviewed PrivateNetwork transient service'}


def execute():
    facts = inspect_boundary()
    require(RUNTIME.is_dir() and not RUNTIME.is_symlink() and RUNTIME.stat().st_uid == 0
            and RUNTIME.stat().st_mode & 0o777 == 0o700, 'Require fresh root-only runtime directory')
    require(EVIDENCE.is_dir() and not EVIDENCE.is_symlink() and EVIDENCE.stat().st_uid == 0
            and EVIDENCE.stat().st_mode & 0o777 == 0o700, 'Require fresh root-only evidence directory')
    result_path = EVIDENCE / 'result.json'
    # Exclusive creation before any kernel operation; do not replace old evidence.
    with result_path.open('x') as result_file:
        result = {'started_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                  'boundary': facts, 'complete': False, 'ban_apply_supported': False, 'cases': []}
        observer = guard = None
        try:
            observer = Netfilter()
            require(observer.tables() == [], 'Preserve existing table(s); fresh namespace required')
            guard = Netfilter(); guard.create(GUARD)
            result['cases'].append(run_case(observer, guard, 'close'))
            result['cases'].append(run_case(observer, guard, 'sigkill'))
            guard.close(); guard = None
            end = time.monotonic() + DEADLINE
            while True:
                rows = observer.tables()
                if not rows:
                    break
                require(time.monotonic() < end, 'Final empty-table verification failed')
                time.sleep(0.05)
            result['after_tables'] = rows
            result['complete'] = True
        except BaseException as error:
            result['error'] = repr(error)
            raise
        finally:
            if guard is not None:
                guard.close()
            if observer is not None:
                observer.close()
            result['ended_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
            result_file.write(json.dumps(result, indent=2)); result_file.flush()
        print(json.dumps(result, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--offline-plan', action='store_true')
    group.add_argument('--execute-isolated-owner-probe', action='store_true')
    args = parser.parse_args()
    if args.offline_plan:
        print(json.dumps(plan(), indent=2))
    else:
        execute()


if __name__ == '__main__':
    main()
