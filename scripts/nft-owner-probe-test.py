#!/usr/bin/env python3
"""Offline protocol/failure tests. Never instantiate a real Netlink socket."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import socket
import stat
import struct
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).with_name('nft-owner-probe.py')
SPEC = importlib.util.spec_from_file_location('nft_owner_probe', SOURCE)
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)
REAL_FSTAT = os.fstat
BOOT_ID = '12345678-1234-1234-1234-123456789abc'


def reference():
    return dict(version=1, boot_id=BOOT_ID, init_netns=[4, 10],
                init_pidns=[4, 30], init_userns=[4, 40])


def fixture_root_stat(fd):
    """Only the test's fstat reply is modeled; no chown or sudo is performed."""
    actual = REAL_FSTAT(fd)
    keys = ['st_dev', 'st_ino', 'st_mode', 'st_uid', 'st_gid', 'st_nlink', 'st_size',
            'st_mtime_ns', 'st_ctime_ns']
    fields = {key: getattr(actual, key) for key in keys}
    fields['st_uid'] = 0
    return SimpleNamespace(**fields)


def table(name=probe.GUARD, port=100, flags=probe.OWNER, use=0, generation=19):
    return (struct.pack('!BBH', 1, 0, generation)
            + probe.attribute(1, name.encode() + b'\0')
            + probe.attribute(2, struct.pack('!I', flags))
            + probe.attribute(3, struct.pack('!I', use))
            + probe.attribute(7, struct.pack('!I', port)))


def ack(sequence, code=0, original_kind=probe.NFT_NEW_TABLE, original_sequence=None):
    header = probe.HEADER.pack(16, original_kind, probe.REQUEST,
                              sequence if original_sequence is None else original_sequence, 0)
    return probe.message(probe.NL_ERROR, 0, sequence, struct.pack('=i', code) + header)


class FakeSocket:
    def __init__(self, data):
        self.data = list(data)
        self.sent = []
    def settimeout(self, _):
        pass
    def recvmsg(self, _):
        if not self.data:
            raise TimeoutError('simulated missing reply')
        return self.data.pop(0), [], 0, (0, 0)
    def sendto(self, data, address):
        self.sent.append((data, address))
        return len(data)


def client(*replies):
    peer = probe.Netfilter.__new__(probe.Netfilter)
    peer.sock = FakeSocket(replies)
    peer.port = 100
    peer.sequence = 100
    return peer


def boundary():
    return dict(platform='linux', uid=0, netns=[4, 20], init_netns=[4, 10],
                pidns=[4, 30], init_pidns=[4, 30], userns=[4, 40], init_userns=[4, 40],
                cgroup='/system.slice/' + probe.UNIT, cap_eff=probe.NET_ADMIN,
                cap_prm=probe.NET_ADMIN, cap_bnd=probe.NET_ADMIN, no_new_privs=1,
                interfaces=['lo'], nf_tables_loaded=True)


class BoundaryTests(unittest.TestCase):
    def test_preconditions_refuse_host_extra_privileges_devices_and_missing_module(self):
        probe.validate_boundary(boundary())
        for field, bad in [('platform', 'darwin'), ('uid', 1000), ('netns', [4, 10]),
                           ('pidns', [4, 99]), ('userns', [4, 99]), ('cgroup', '/'),
                           ('cap_eff', probe.NET_ADMIN | 1), ('cap_prm', 0), ('cap_bnd', 0xffff),
                           ('no_new_privs', 0), ('interfaces', ['lo', 'ens5']), ('nf_tables_loaded', False)]:
            with self.subTest(field=field):
                facts = boundary(); facts[field] = bad
                with self.assertRaises(probe.Refusal):
                    probe.validate_boundary(facts)

    def test_create_batch_contains_only_fixed_owner_table(self):
        messages = probe.parse_messages(probe.create_owner_batch(probe.OWNED, 100))
        self.assertEqual([m[0] for m in messages], [probe.BATCH_BEGIN, probe.NFT_NEW_TABLE, probe.BATCH_END])
        self.assertEqual([m[2] for m in messages], [100, 101, 102])
        self.assertEqual(messages[1][1], probe.REQUEST | probe.ACK | probe.CREATE_EXCL)
        self.assertEqual(probe.parse_table(messages[1][4]),
                         {'family': 1, 'name': probe.OWNED, 'generation': 0, 'flags': probe.OWNER})
        self.assertEqual(messages[0][4], struct.pack('!BBH', 0, 0, 10))

    def test_names_and_sequence_cannot_inject_another_command(self):
        for name in ['', 'filter', 'vno_owned; flush ruleset', 'vno_owned\0x', '../vno_owned']:
            with self.assertRaises(probe.Refusal):
                probe.create_owner_batch(name, 100)
        for sequence in [0, 2**32 - 2, 2**32]:
            with self.assertRaises(probe.Refusal):
                probe.create_owner_batch(probe.OWNED, sequence)

    def test_nonzero_nfgen_reply_generation_is_valid(self):
        row = probe.parse_table(table(generation=255))
        self.assertEqual(row['generation'], 255)
        probe.verify_table(row, probe.GUARD, 100)

    def test_table_attributes_malformed_or_duplicate_are_not_absence(self):
        for payload in [b'', b'\x01\0', table()[:-1],
                        table() + probe.attribute(2, struct.pack('!I', 2)),
                        struct.pack('!BBH', 1, 1, 0) + probe.attribute(1, b'x\0'),
                        struct.pack('!BBH', 1, 0, 0) + probe.attribute(1, b'x\0y\0')]:
            with self.subTest(size=len(payload)), self.assertRaises(probe.Refusal):
                probe.parse_table(payload)

    def test_wrong_flags_owner_family_or_use_refuse(self):
        good = probe.parse_table(table())
        for key, value in [('family', 2), ('name', 'foreign'), ('flags', 0),
                           ('flags', 6), ('owner', 101), ('use', 1)]:
            row = copy.copy(good); row[key] = value
            with self.assertRaises(probe.Refusal):
                probe.verify_table(row, probe.GUARD, 100)

    def test_sender_truncation_and_interrupted_dump_refuse(self):
        valid = ack(101)
        for sender, flags in [((100, 0), 0), ((0, 1), 0), ((0, 0), socket.MSG_TRUNC),
                              ((0, 0), socket.MSG_CTRUNC)]:
            with self.assertRaises(probe.Refusal):
                probe.parse_messages(valid, sender, flags)
        for data in [b'', b'x', valid[:-1],
                     probe.message(probe.NL_DONE, probe.DUMP_INTR, 101, struct.pack('=i', 0)),
                     probe.message(probe.NL_OVERRUN, 0, 101, b'')]:
            with self.assertRaises(probe.Refusal):
                probe.parse_messages(data)

    def test_create_uses_one_send_and_ack_bound_to_original_request(self):
        peer = client(ack(101)); peer.create(probe.OWNED)
        self.assertEqual(len(peer.sock.sent), 1)
        self.assertEqual(peer.sequence, 103)
        for bad in [ack(999), ack(101, original_sequence=100), ack(101, original_kind=probe.NFT_GET_TABLE),
                    ack(101, code=-1), ack(101, code=1),
                    probe.message(probe.NL_ERROR, 0, 101, b'\0' * 4)]:
            peer = client(bad)
            with self.assertRaises(probe.Refusal):
                peer.create(probe.OWNED)
            self.assertEqual(len(peer.sock.sent), 1)

    def test_ack_loss_does_not_resend(self):
        peer = client()
        with self.assertRaises(TimeoutError):
            peer.create(probe.OWNED)
        self.assertEqual(len(peer.sock.sent), 1)

    def test_complete_empty_dump_and_all_family_request(self):
        peer = client(probe.message(probe.NL_DONE, probe.MULTI, 100, struct.pack('=i', 0)))
        self.assertEqual(peer.tables(), [])
        request = probe.parse_messages(peer.sock.sent[0][0])[0]
        self.assertEqual(request[0], probe.NFT_GET_TABLE)
        self.assertEqual(request[4], struct.pack('!BBH', 0, 0, 0))

    def test_dump_requires_terminator_no_error_or_generation_mix(self):
        row = probe.message(probe.NFT_NEW_TABLE, probe.MULTI, 100, table())
        done = probe.message(probe.NL_DONE, probe.MULTI, 100, struct.pack('=i', 0))
        self.assertEqual(client(row + done).tables()[0]['name'], probe.GUARD)
        with self.assertRaises(TimeoutError):
            client(row).tables()
        bad_row = probe.message(probe.NFT_NEW_TABLE, probe.MULTI, 100, table(probe.OWNED, generation=20))
        for packets in [(row + bad_row + done,),
                        (ack(100, code=-1, original_kind=probe.NFT_GET_TABLE),),
                        (probe.message(probe.NL_DONE, probe.MULTI, 100, struct.pack('=i', -1)),)]:
            with self.assertRaises(probe.Refusal):
                client(*packets).tables()

    def test_reply_budget_prevents_unbounded_dump(self):
        message = probe.message(probe.NFT_NEW_TABLE, probe.MULTI, 100, table())
        with self.assertRaises(probe.Refusal):
            client(message * (probe.MAX_MESSAGES + 1)).tables()

    def test_review_command_preserves_isolation_limits_and_evidence(self):
        command = probe.unit_command()
        self.assertEqual(command[:4], ['/usr/bin/systemd-run', '--unit=' + probe.UNIT,
                                      '--service-type=exec', '--wait'])
        properties = dict(arg[len('--property='):].split('=', 1)
                          for arg in command if arg.startswith('--property='))
        self.assertEqual(properties['PrivateNetwork'], 'yes')
        self.assertEqual(properties['CapabilityBoundingSet'], 'CAP_NET_ADMIN')
        self.assertEqual(properties['RestrictNamespaces'], 'yes')
        self.assertEqual(properties['RestrictAddressFamilies'], 'AF_UNIX AF_NETLINK')
        self.assertEqual(properties['ReadWritePaths'], str(probe.EVIDENCE))
        self.assertEqual(properties['RuntimeMaxSec'], '60s')
        self.assertEqual(properties['TimeoutStopSec'], '5s')
        self.assertEqual(properties['Restart'], 'no')
        self.assertEqual(properties['TasksMax'], '8')
        self.assertEqual(command[-5:], ['/usr/bin/python3', '-I', '-B',
                                       str(probe.RUNTIME / 'nft-owner-probe.py'),
                                       '--execute-isolated-owner-probe'])
        self.assertNotIn('RuntimeDirectory', properties)
        self.assertNotIn('NetworkNamespacePath', properties)

    def test_owner_liveness_observation_cannot_reap_pid(self):
        # waitid is stubbed; no fork, signal or real wait in offline tests.
        with patch.object(probe.os, 'waitid', return_value=None, create=True) as wait:
            # Darwin may lack WNOWAIT; the real execution remains Linux-only.
            with patch.object(probe.os, 'WNOWAIT', 0x1000000, create=True):
                probe.require_child_alive(123)
                self.assertTrue(wait.call_args.args[2] & probe.os.WNOWAIT)
                wait.return_value = object()
                with self.assertRaises(probe.Refusal):
                    probe.require_child_alive(123)

    def test_offline_cli_has_no_implicit_live_mode(self):
        p = subprocess.run([sys.executable, '-I', '-B', str(SOURCE), '--offline-plan'],
                           capture_output=True, text=True, timeout=10)
        self.assertEqual(p.returncode, 0)
        plan = json.loads(p.stdout)
        self.assertFalse(plan['live_executed'])
        self.assertFalse(plan['ban_apply_supported'])
        self.assertEqual(plan['chain_rule_set_count'], 0)
        self.assertEqual(plan['max_tables'], 2)
        self.assertEqual(plan['host_reference']['owner_uid'], 0)
        self.assertEqual(plan['host_reference']['mode'], '0400')
        self.assertFalse(plan['host_reference']['pid_1_access_in_probe'])
        self.assertTrue(plan['host_reference']['same_boot_required'])
        for args in [[], ['--apply'], ['--table', 'filter'], ['--host-reference', '/tmp/other'],
                     ['--offline-plan', '--execute-isolated-owner-probe']]:
            p = subprocess.run([sys.executable, '-I', '-B', str(SOURCE), *args], capture_output=True, timeout=10)
            self.assertEqual(p.returncode, 2)


class ReferenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Keep new test fixtures as evidence. No automatic directory/file deletion.
        cls.fixtures = Path(tempfile.mkdtemp(prefix='nft-owner-reference-tests-',
                                           dir=SOURCE.parent.parent / 'artifacts'))
        cls.fixtures.chmod(0o700)
        print('REFERENCE_FIXTURES=' + str(cls.fixtures), file=sys.stderr)

    def fixture(self, raw=None, mode=0o400):
        directory = Path(tempfile.mkdtemp(prefix='case-', dir=self.fixtures))
        if raw is not None:
            target = directory / probe.HOST_REFERENCE.name
            with target.open('xb') as stream:
                stream.write(raw)
            target.chmod(mode)
        return directory

    def load(self, directory, metadata=fixture_root_stat):
        with patch.object(probe, 'RUNTIME', directory), patch.object(probe.os, 'fstat', side_effect=metadata):
            return probe.load_host_reference(BOOT_ID)

    def test_reference_schema_and_same_boot(self):
        good = reference()
        self.assertEqual(probe.parse_host_reference(json.dumps(good).encode(), BOOT_ID), good)
        for key, value in [('version', True), ('version', 2), ('boot_id', 5), ('boot_id', BOOT_ID.upper()),
                           ('boot_id', '00000000-0000-0000-0000-000000000000'),
                           ('init_netns', [True, 10]), ('init_pidns', [4, 0]),
                           ('init_userns', [4, 2**64]), ('init_netns', [4, '10']),
                           ('init_pidns', [4]), ('init_userns', {'dev': 4, 'ino': 40})]:
            with self.subTest(key=key, value=value):
                bad = dict(good); bad[key] = value
                with self.assertRaises(probe.Refusal):
                    probe.parse_host_reference(json.dumps(bad).encode(), BOOT_ID)
        for bad in [{}, {**good, 'extra': 1}, {k: v for k, v in good.items() if k != 'init_netns'}]:
            with self.assertRaises(probe.Refusal):
                probe.parse_host_reference(json.dumps(bad).encode(), BOOT_ID)

    def test_reference_json_duplicates_corruption_and_size(self):
        encoded = json.dumps(reference()).encode()
        for raw in [b'', b'x' * (probe.MAX_REFERENCE_BYTES + 1), b'\xff', b'{', b'[]', b'null',
                    encoded + b'{}', b'{"version":1,' + encoded[1:],
                    b'[' * 1500 + b']' * 1500]:
            with self.subTest(size=len(raw)), self.assertRaises(probe.Refusal):
                probe.parse_host_reference(raw, BOOT_ID)

    def test_file_is_anchored_read_only_and_fds_are_closed(self):
        directory = self.fixture(json.dumps(reference()).encode())
        real_open, real_close = os.open, os.close
        opened, closed = [], []
        def opening(path, flags, *args, **kwargs):
            self.assertTrue(flags & os.O_NOFOLLOW)
            self.assertTrue(flags & os.O_CLOEXEC)
            self.assertFalse(flags & (os.O_CREAT | os.O_TRUNC | os.O_WRONLY | os.O_RDWR))
            if path == probe.HOST_REFERENCE.name:
                self.assertEqual(kwargs['dir_fd'], opened[0])
                self.assertTrue(flags & os.O_NONBLOCK)
            fd = real_open(path, flags, *args, **kwargs)
            opened.append(fd); return fd
        def closing(fd):
            closed.append(fd); return real_close(fd)
        with patch.object(probe.os, 'open', side_effect=opening), patch.object(probe.os, 'close', side_effect=closing):
            self.assertEqual(self.load(directory), reference())
        self.assertEqual(sorted(opened), sorted(closed))

    def test_reference_owner_parent_mode_file_mode_and_links_refuse(self):
        raw = json.dumps(reference()).encode()
        for mode in [0o600, 0o444, 0o440, 0o000, 0o4400]:
            with self.subTest(mode=oct(mode)), self.assertRaises(probe.Refusal):
                self.load(self.fixture(raw, mode))
        for field, value, directory_only in [('st_uid', 1000, True), ('st_uid', 1000, False),
                                             ('st_nlink', 2, False)]:
            def changed(fd):
                meta = fixture_root_stat(fd)
                if stat.S_ISDIR(meta.st_mode) == directory_only:
                    setattr(meta, field, value)
                return meta
            with self.subTest(field=field, directory=directory_only), self.assertRaises(probe.Refusal):
                self.load(self.fixture(raw), changed)
        directory = self.fixture(raw); directory.chmod(0o770)
        with self.assertRaises(probe.Refusal): self.load(directory)
        directory = self.fixture(raw)
        os.link(directory / probe.HOST_REFERENCE.name, directory / 'extra-link')
        with self.assertRaises(probe.Refusal): self.load(directory)

    def test_missing_symlink_fifo_directory_and_oversize_refuse(self):
        raw = json.dumps(reference()).encode()
        missing = self.fixture()
        symlink = self.fixture()
        (symlink / probe.HOST_REFERENCE.name).symlink_to(self.fixture(raw) / probe.HOST_REFERENCE.name)
        fifo = self.fixture(); os.mkfifo(fifo / probe.HOST_REFERENCE.name, 0o400)
        subdir = self.fixture(); (subdir / probe.HOST_REFERENCE.name).mkdir(mode=0o400)
        bad_parent = self.fixtures / ('symlink-' + missing.name); bad_parent.symlink_to(self.fixture(raw))
        for directory in [missing, symlink, fifo, subdir, bad_parent,
                          self.fixture(b''), self.fixture(b'x' * (probe.MAX_REFERENCE_BYTES + 1))]:
            with self.subTest(path=str(directory)), self.assertRaises(probe.Refusal): self.load(directory)

    def test_changed_or_short_reference_read_refuses(self):
        raw = json.dumps(reference()).encode()
        directory = self.fixture(raw)
        file_stats = 0
        def changed(fd):
            nonlocal file_stats
            meta = fixture_root_stat(fd)
            if stat.S_ISREG(meta.st_mode):
                file_stats += 1
                if file_stats == 2: meta.st_mtime_ns += 1
            return meta
        with self.assertRaises(probe.Refusal): self.load(directory, changed)
        with patch.object(probe.os, 'read', return_value=b''):
            with self.assertRaises(probe.Refusal): self.load(directory)
        def oversized(fd):
            meta = fixture_root_stat(fd)
            if stat.S_ISREG(meta.st_mode): meta.st_size = probe.MAX_REFERENCE_BYTES + 1
            return meta
        with patch.object(probe.os, 'read', side_effect=AssertionError('must not read')):
            with self.assertRaises(probe.Refusal): self.load(directory, oversized)

    def test_file_denial_has_no_fallback_and_closes_directory_fd(self):
        directory = self.fixture(json.dumps(reference()).encode())
        real_open, real_close = os.open, os.close
        opened, closed = [], []
        def denied(path, flags, *args, **kwargs):
            if path == probe.HOST_REFERENCE.name: raise PermissionError(13, 'simulated denial')
            fd = real_open(path, flags, *args, **kwargs); opened.append(fd); return fd
        def closing(fd): closed.append(fd); return real_close(fd)
        with patch.object(probe.os, 'open', side_effect=denied), patch.object(probe.os, 'close', side_effect=closing):
            with self.assertRaises(probe.Refusal): self.load(directory)
        self.assertEqual(opened, closed)


class InspectionTests(unittest.TestCase):
    def inspect(self, *, host_net=False, bad_pid=False, bad_user=False, extra_cap=False, missing_ref=False):
        facts = boundary()
        values = {'/proc/self/ns/net': facts['init_netns'] if host_net else facts['netns'],
                  '/proc/self/ns/pid': [4, 99] if bad_pid else facts['pidns'],
                  '/proc/self/ns/user': [4, 99] if bad_user else facts['userns']}
        def identity(path):
            if path not in values: raise PermissionError('PID 1 direct reference denied')
            return values[path]
        cap = probe.NET_ADMIN | 1 if extra_cap else probe.NET_ADMIN
        texts = {'/proc/self/status': f'CapEff:\t{cap:x}\nCapPrm:\t{cap:x}\nCapBnd:\t{cap:x}\nNoNewPrivs:\t1\n',
                 '/proc/self/cgroup': '0::/system.slice/' + probe.UNIT + '\n',
                 '/proc/sys/kernel/random/boot_id': BOOT_ID + '\n'}
        with patch.object(probe.sys, 'platform', 'linux'), patch.object(probe.os, 'geteuid', return_value=0), \
             patch.object(probe.Path, 'read_text', lambda path: texts[str(path)]), \
             patch.object(probe.Path, 'is_dir', return_value=True), \
             patch.object(probe, 'load_host_reference', create=True, return_value=reference(),
                          side_effect=probe.Refusal('missing reference') if missing_ref else None) as loader, \
             patch.object(probe, 'namespace_id', side_effect=identity) as ns, \
             patch.object(probe.socket, 'socket', side_effect=AssertionError('no kernel socket')), \
             patch.object(probe.subprocess, 'run', return_value=SimpleNamespace(
                 returncode=0, stdout='[{"ifname":"lo"}]')) as command:
            try:
                result = probe.inspect_boundary()
            except probe.Refusal:
                command.assert_not_called()
                raise
            loader.assert_called_once_with(BOOT_ID)
            self.assertEqual([call.args[0] for call in ns.call_args_list], list(values))
            command.assert_called_once()
            return result

    def test_inspection_uses_reference_and_only_self_namespaces(self):
        self.assertEqual(self.inspect()['init_netns'], reference()['init_netns'])

    def test_invalid_boundary_refuses_before_ip_or_socket(self):
        for flags in [dict(host_net=True), dict(bad_pid=True), dict(bad_user=True),
                      dict(extra_cap=True), dict(missing_ref=True)]:
            with self.subTest(flags=flags), self.assertRaises(probe.Refusal): self.inspect(**flags)


if __name__ == '__main__':
    unittest.main(verbosity=2)
