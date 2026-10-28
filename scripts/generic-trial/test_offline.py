#!/usr/bin/env python3
"""Retained fixtures only; all NIC mutations/cloud metadata are mocked."""
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import struct
import tarfile
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


f = load('features_test', 'features.py')
p = load('package_test', 'package.py')
n = load('nic_test', 'nic-check.py')
a = load('access_test', 'access-check.py')
EVIDENCE = Path(os.environ['VEIL_GENERIC_EVIDENCE']).resolve(strict=True)


def features():
    return [{'index': i, 'name': name, 'available': True, 'wanted': False,
             'active': False, 'never_changed': False}
            for i, name in enumerate(['rx-lro', 'rx-gro-hw', 'rx-gro'])]


def snapshot(rows):
    return {'identity': {'ifindex': 2, 'boot': 'same'}, 'features': rows, 'link': {}}


class Offline(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix='case-', dir=EVIDENCE))

    def test_off_state_no_set(self):
        rows = features()
        self.assertEqual(f.off_args(rows, copy.deepcopy(rows)), [])
        self.assertIsNone(f.set_command([]))

    def test_active_and_wanted_are_independently_required_off(self):
        for target in range(2):
            for key in ('active', 'wanted'):
                rows = features()
                rows[target][key] = True
                with self.assertRaises(ValueError):
                    f.admit_off(rows)
                self.assertEqual(f.off_args(rows, copy.deepcopy(rows)), [f.ALIASES[rows[target]['name']], 'off'])

    def test_fixed_off_is_noop_but_fixed_on_rejects_entire_plan(self):
        rows = features()
        rows[0]['available'] = False
        rows[0]['never_changed'] = True
        f.admit_off(rows)
        rows[0]['wanted'] = True
        rows[1]['active'] = True
        with self.assertRaises(ValueError):
            f.off_args(rows, copy.deepcopy(rows))

    def test_two_features_one_command_and_no_shell_text(self):
        rows = features()
        for r in rows[:2]:
            r.update(wanted=True, active=True)
        self.assertEqual(f.set_command(f.off_args(rows, rows)),
                         ['/usr/sbin/ethtool', '-K', 'ens4', 'lro', 'off', 'rx-gro-hw', 'off'])
        for args in [['gro', 'off'], ['lro;reboot', 'off'], ['lro', 'off', 'lro', 'on'], ['lro', 0], ['lro']]:
            with self.assertRaises(ValueError):
                f.set_command(args)

    def test_unknown_incomplete_duplicate_or_nonboolean_features_rejected(self):
        for change in ('missing', 'duplicate', 'number', 'unknown', 'index'):
            rows = features()
            if change == 'missing': rows.pop(0)
            if change == 'duplicate': rows[1]['name'] = rows[0]['name']
            if change == 'number': rows[1]['wanted'] = 0
            if change == 'unknown': rows[1]['extra'] = False
            if change == 'index': rows[1]['index'] = True
            with self.assertRaises(ValueError): f.validate(rows)

    def test_feature_decode_word_boundaries_and_padding(self):
        names = [''] * 64
        names[31], names[32] = f.TARGETS
        raw = b''.join(s.encode().ljust(32, b'\0') for s in names)
        blocks = [0] * 8
        rows = f.decode(raw, blocks, 64)
        f.admit_off(rows)
        blocks[5] = 1
        with self.assertRaises(ValueError): f.admit_off(f.decode(raw, blocks, 64))
        for data, words, count in [(raw, [], 64), (raw, blocks, 257), (b'x' * 32, [0] * 4, 1)]:
            with self.assertRaises(ValueError): f.decode(data, words, count)

    def test_restore_uses_original_wanted_not_active(self):
        old = features()
        old[1]['wanted'] = True  # active originally false
        current = features()
        self.assertEqual(f.restore_args(old, current), ['rx-gro-hw', 'on'])
        self.assertEqual(f.restore_args(old, old), [])

    def test_unrelated_or_new_target_changes_refuse_restore(self):
        old = features()
        for key, index in [('wanted', 2), ('active', 2), ('active', 0), ('wanted', 1)]:
            current = copy.deepcopy(old)
            current[index][key] = True
            with self.assertRaises(ValueError): f.restore_args(old, current)

    def test_prepare_rejects_stale_b0_and_layout_drift(self):
        old, current = features(), features()
        current[2]['wanted'] = True
        with self.assertRaises(ValueError): f.off_args(old, current)
        current = features()
        current[0]['available'] = False
        with self.assertRaises(ValueError): f.same_layout(old, current)

    def test_partial_prepare_or_unrelated_post_change_is_failure(self):
        old, current = features(), features()
        current[0]['wanted'] = True
        with self.assertRaises(ValueError): f.off_result(old, current)
        current = features()
        current[2]['active'] = True
        with self.assertRaises(ValueError): f.off_result(old, current)

    def test_budget_and_case_order_are_fixed(self):
        n.require_case_start('normal', 1499, False)
        n.require_case_start('kill', 1919, True)
        for case, elapsed, stopped in [('normal', 1500, False), ('kill', 1920, True), ('kill', 1000, False), ('other', 0, True)]:
            with self.assertRaises(ValueError): n.require_case_start(case, elapsed, stopped)

    def test_existing_xdp_preserved(self):
        n.vacant({})
        for xdp in [{'attached': [1]}, {'prog': {'id': 1}}]:
            with self.assertRaises(ValueError): n.vacant({'xdp': xdp})

    def test_prepare_marks_attempt_before_set_and_never_retries_unknown_result(self):
        old = features()
        old[1].update(active=True, wanted=True)
        before = snapshot(old)
        def save(name, data):
            with (self.root / name).open('x') as out: json.dump(data, out)
        def failed(command):
            self.assertTrue((self.root / 'prepare-attempt.json').exists())
            raise RuntimeError('outcome unknown')
        with patch.object(n, 'before_set', return_value=({'t0': 0}, before, before)), \
                patch.object(n.time, 'time', return_value=100), patch.object(n, 'save', side_effect=save), \
                patch.object(n, 'run', side_effect=failed) as runner:
            with self.assertRaises(RuntimeError): n.prepare('unused')
            with self.assertRaises(FileExistsError): n.prepare('unused')
            self.assertEqual(runner.call_count, 1)

    def test_unsupported_prepare_does_not_execute_or_write_attempt(self):
        old = features()
        old[0].update(wanted=True, available=False)
        before = snapshot(old)
        with patch.object(n, 'before_set', return_value=({'t0': 0}, before, before)), \
                patch.object(n.time, 'time', return_value=100), patch.object(n, 'save') as save, \
                patch.object(n, 'run') as runner:
            with self.assertRaises(ValueError): n.prepare('unused')
            save.assert_not_called()
            runner.assert_not_called()

    def test_restore_verifies_full_b0_after_single_set(self):
        old = features()
        old[1].update(active=True, wanted=True)
        before, current = snapshot(old), snapshot(features())
        with patch.object(n, 'ROOT', self.root), patch.object(n, 'before_set', return_value=({}, before, current)), \
                patch.object(n, 'read', return_value={'identity': before['identity']}), \
                patch.object(n, 'capture', return_value=before), patch.object(n, 'save') as save, \
                patch.object(n, 'run') as runner:
            (self.root / 'prepare-attempt.json').write_text('fixture')
            result = n.restore('unused')
            self.assertTrue(result['restored'])
            runner.assert_called_once_with(['/usr/sbin/ethtool', '-K', 'ens4', 'rx-gro-hw', 'on'])
            self.assertEqual(save.call_args_list[0].args[0], 'restore-attempt.json')

    def test_restore_drift_never_runs_set(self):
        old, current = snapshot(features()), snapshot(features())
        current['features'][2]['wanted'] = True
        with patch.object(n, 'ROOT', self.root), patch.object(n, 'before_set', return_value=({}, old, current)), \
                patch.object(n, 'read', return_value={'identity': old['identity']}), patch.object(n, 'run') as runner:
            (self.root / 'prepare-attempt.json').write_text('fixture')
            with self.assertRaises(ValueError): n.restore('unused')
            runner.assert_not_called()

    def test_profile_and_driver_preserve_limits_without_automatic_restart(self):
        profile = Path(__file__).with_name('profile.service').read_text()
        for value in ['RuntimeMaxSec=180s', 'TimeoutStartSec=10s', 'TimeoutStopSec=10s', 'Restart=no', 'NoNewPrivileges=yes']:
            self.assertIn(value, profile)
        driver = Path(__file__).with_name('driver.py').read_text()
        for value in ["'--xdp-mode', 'generic'", 'time.sleep(3.1)', "ttl=3", "'sigkill_detaches_before_restart'", "'systemctl', 'start', unit"]:
            self.assertIn(value, driver)
        self.assertNotIn('sigkill_auto_restart_ready', driver)

    def test_native_help_failure_and_timeout_are_not_bypassed(self):
        with patch.object(a, 'access_only', return_value={}), patch.object(a.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, '', 'denied')):
            with self.assertRaises(ValueError): a.user_gate(self.root, 'unused')
        with patch.object(a, 'access_only', return_value={}), patch.object(a.subprocess, 'run', side_effect=subprocess.TimeoutExpired([], 3)):
            with self.assertRaises(subprocess.TimeoutExpired): a.user_gate(self.root, 'unused')

    def test_nonroot_or_wrong_owner_context_never_starts_command(self):
        with patch.object(n.os, 'geteuid', return_value=1000), patch.object(n, 'run') as runner:
            with self.assertRaises(ValueError): n.start('unused', 'normal')
            runner.assert_not_called()
        scope = {'binary_sha256': 'a' * 64, 'instance': n.VM, 'xdp_mode': 'generic', 'management_source': '111.217.200.148'}
        (self.root / 'scope.json').write_text(json.dumps(scope))
        manifest = {'files': {'veil-local-ban': {'sha256': 'a' * 64}}}
        state = {'metadata': {'id': '123'}, 'boot': 'boot', 't0': 10}
        def path(v):
            if v == '/proc/sys/kernel/random/boot_id': return SimpleNamespace(read_text=lambda: 'boot\n')
            return Path(v)
        with patch.object(n, 'ROOT', self.root), patch.object(n.os, 'geteuid', return_value=0), \
                patch.object(n.access, 'check_files', return_value=manifest), patch.object(n, 'read', return_value=state), \
                patch.object(n, 'Path', side_effect=path), patch.object(n.time, 'time', return_value=100), \
                patch.object(n, 'metadata', return_value={'id': '124'}), patch.object(n, 'run') as runner:
            with self.assertRaises(ValueError): n.start('unused', 'normal')
            runner.assert_not_called()

    def test_metadata_is_bounded_and_rejects_other_vm(self):
        response = SimpleNamespace(status=200, getheader=lambda _: 'Google', read=lambda _: b'wrong')
        conn = SimpleNamespace(request=lambda *a, **k: None, getresponse=lambda: response, close=lambda: None)
        with patch.object(n.http.client, 'HTTPConnection', return_value=conn):
            with self.assertRaises(ValueError): n.metadata()

    def test_readonly_route_gate_accepts_only_kernel_complete_replies(self):
        class Reply:
            def __init__(self, bad=None): self.bad, self.kind = bad, None
            def __enter__(self): return self
            def __exit__(self, *_): return None
            def bind(self, _): pass
            def settimeout(self, _): pass
            def sendto(self, data, _):
                self.kind = struct.unpack_from('=H', data, 4)[0]
                self.assert_get()
            def assert_get(self):
                if self.kind not in (18, 38, 46): raise AssertionError('SET/NEW not allowed')
            def recvmsg(self, _):
                ty, body = (16, bytes(16)) if self.kind == 18 else (3, bytes(4))
                data = struct.pack('=IHHII', 16 + len(body), ty, 0x10 if self.bad == 'interrupted' else 0,
                                   2 if self.bad == 'sequence' else 1, 0) + body
                return data, [], f.socket.MSG_TRUNC if self.bad == 'truncated' else 0, (99 if self.bad == 'sender' else 0, 0)
        for bad in (None, 'sequence', 'sender', 'truncated', 'interrupted'):
            with patch.object(f.sys, 'platform', 'linux'), \
                    patch.object(f.socket, 'AF_NETLINK', 16, create=True), \
                    patch.object(f.socket, 'NETLINK_ROUTE', 0, create=True), \
                    patch.object(f.socket, 'SOCK_CLOEXEC', 0o2000000, create=True), \
                    patch.object(f.socket, 'if_nametoindex', return_value=1), \
                    patch.object(f.socket, 'socket', side_effect=lambda *_: Reply(bad)):
                if bad:
                    with self.assertRaises(ValueError): f.route_get_smoke()
                else:
                    self.assertEqual(f.route_get_smoke()['read_only_loopback_gets'], [18, 38, 46])

    def test_unknown_owner_files_are_not_modified(self):
        path = self.root / 'unknown.json'
        path.write_bytes(b'preserve')
        with patch.object(n, 'ROOT', self.root):
            with self.assertRaises(ValueError): n.read('unknown.json')
        self.assertEqual(path.read_bytes(), b'preserve')

    def test_runtime_dynamic_user_names_fit_system_limit(self):
        self.assertLessEqual(len(n.ROOT.name + '-n'), 31)
        self.assertLessEqual(len(n.ROOT.name + '-k'), 31)

    def archive(self, edit=None):
        bodies = {name: name.encode() for name in p.MODES if name != 'manifest.json'}
        p.SOURCES = {name: ('unused', p.digest(body)) for name, body in bodies.items()}
        manifest = {'version': 2, 'files': {name: {'sha256': p.digest(body), 'bytes': len(body), 'mode': p.MODES[name]} for name, body in bodies.items()}}
        bodies['manifest.json'] = json.dumps(manifest).encode()
        entries = []
        for name, body in bodies.items():
            m = tarfile.TarInfo(name)
            m.mode, m.size = p.MODES[name], len(body)
            entries.append([m, body])
        if edit: edit(entries)
        target = self.root / 'fixture.tar.gz'
        with tarfile.open(target, 'w:gz') as out:
            for m, body in entries: out.addfile(m, io.BytesIO(body))
        return target

    def test_all_assets_and_catalog_are_pinned(self):
        archive = self.archive()
        self.assertEqual(len(p.verify(archive)['files']), 9)
        p.SOURCES['features.py'] = ('unused', '0' * 64)
        with self.assertRaises(ValueError): p.verify(archive)
        p.SOURCES = {}
        with self.assertRaises(ValueError): p.verify(archive)

    def test_unsafe_archive_metadata_rejected(self):
        for kind in ('mode', 'owner', 'symlink', 'duplicate', 'escape', 'pax', 'missing'):
            def edit(items):
                if kind == 'mode': items[0][0].mode = 0o700
                if kind == 'owner': items[0][0].uid = 1000
                if kind == 'symlink': items[0][0].type = tarfile.SYMTYPE
                if kind == 'duplicate': items.append(copy.deepcopy(items[0]))
                if kind == 'escape': items[0][0].name = '../outside'
                if kind == 'pax': items[0][0].pax_headers = {'SCHILY.acl.access': 'user::rwx'}
                if kind == 'missing': items.pop(0)
            with self.subTest(kind=kind):
                with self.assertRaises(ValueError): p.verify(self.archive(edit))

    def test_bad_catalog_hash_and_existing_output_preserved(self):
        target = self.root / 'catalog.json'
        target.write_text('{}')
        with self.assertRaises(ValueError): p.load_catalog(target, '0' * 64)
        out = self.root / 'existing.tar.gz'
        out.write_bytes(b'preserve')
        with self.assertRaises(ValueError): p.build(out)
        self.assertEqual(out.read_bytes(), b'preserve')

    def test_native_gate_requires_unmodified_full_regression_success(self):
        help_result = subprocess.CompletedProcess([], 0, 'Usage: serve default, generic', '')
        for stdout, code in [('50 passed; 1 failed; 1 ignored', 101), ('50 passed; 0 failed; 2 ignored', 0), ('51 passed; 0 failed; 1 ignored', 0)]:
            with patch.object(a, 'access_only', return_value={}), \
                    patch.object(a.subprocess, 'run', side_effect=[help_result, help_result, subprocess.CompletedProcess([], code, stdout, '')]):
                if code == 0 and stdout.startswith('51'):
                    self.assertEqual(a.user_gate(self.root, 'unused', native=True)['native_kernel_regression_exit'], 0)
                else:
                    with self.assertRaises(ValueError): a.user_gate(self.root, 'unused', native=True)


if __name__ == '__main__':
    unittest.main()
