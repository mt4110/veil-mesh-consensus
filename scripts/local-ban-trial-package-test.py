#!/usr/bin/env python3
"""Offline regression; creates and retains only its own evidence fixtures."""
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


p = load('trial_package', 'local-ban-trial-package.py')
a = load('trial_access', 'local-ban-trial-access.py')
EVIDENCE = Path(os.environ['VEIL_PACKAGE_TEST_EVIDENCE']).resolve(strict=True)


class PackageRegression(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix='case-', dir=EVIDENCE))
        self.files = {name: name.encode() for name in p.MODES if name != 'manifest.json'}
        self.catalog = {name: ('unused', p.digest(data)) for name, data in self.files.items()}

    def archive(self, edit=None, duplicate=False, extra=False):
        manifest = {'version': 1, 'files': {name: {'sha256': p.digest(body), 'bytes': len(body),
                                                 'mode': p.MODES[name]} for name, body in self.files.items()}}
        files = {**self.files, 'manifest.json': json.dumps(manifest).encode()}
        entries = []
        for name, body in files.items():
            member = tarfile.TarInfo(name)
            member.mode = p.MODES[name]
            member.size = len(body)
            entries.append([member, body])
        if edit:
            edit(entries)
        if duplicate:
            entries.append(copy.deepcopy(entries[0]))
        if extra:
            entries.append([tarfile.TarInfo('../outside'), b''])
        output = self.root / 'fixture.tar.gz'
        with tarfile.open(output, 'w:gz') as t:
            for member, body in entries:
                t.addfile(member, io.BytesIO(body))
        return output

    def verify(self, path):
        with patch.object(p, 'SOURCES', self.catalog):
            return p.verify(path)

    def rejected(self, edit=None, **kwargs):
        with self.assertRaises((ValueError, KeyError)):
            self.verify(self.archive(edit, **kwargs))

    def test_canonical_archive_accepted(self):
        self.assertEqual(len(self.verify(self.archive())['files']), 6)

    def test_previous_real_package_rejected(self):
        with self.assertRaisesRegex(ValueError, 'owner/mode'):
            p.verify(p.REPO / 'artifacts/gcp-xdp-live-b6psh2dz/payload-v2.tar.gz')

    def test_binary_0700_rejected(self):
        self.rejected(lambda x: setattr(x[0][0], 'mode', 0o700))

    def test_object_0600_rejected(self):
        self.rejected(lambda x: setattr(x[1][0], 'mode', 0o600))

    def test_nonroot_owner_rejected(self):
        self.rejected(lambda x: setattr(x[0][0], 'uid', 1000))

    def test_nonroot_group_rejected(self):
        self.rejected(lambda x: setattr(x[0][0], 'gid', 1000))

    def test_service_writable_binary_rejected(self):
        self.rejected(lambda x: setattr(x[0][0], 'mode', 0o777))

    def test_setuid_binary_rejected(self):
        self.rejected(lambda x: setattr(x[0][0], 'mode', 0o4755))

    def test_symlink_member_rejected(self):
        self.rejected(lambda x: setattr(x[0][0], 'type', tarfile.SYMTYPE))

    def test_hardlink_member_rejected(self):
        self.rejected(lambda x: setattr(x[0][0], 'type', tarfile.LNKTYPE))

    def test_duplicate_rejected(self):
        self.rejected(duplicate=True)

    def test_path_escape_rejected(self):
        self.rejected(extra=True)

    def test_pax_acl_rejected(self):
        self.rejected(lambda x: setattr(x[0][0], 'pax_headers', {'SCHILY.acl.access': 'user::rwx'}))

    def test_changed_body_rejected(self):
        self.rejected(lambda x: x[0].__setitem__(1, b'X' * len(x[0][1])))

    def test_missing_object_rejected(self):
        self.rejected(lambda x: x.pop(1))

    def test_wrong_pinned_asset_rejected(self):
        archive = self.archive()
        self.catalog['veil-local-ban'] = ('unused', '0' * 64)
        with self.assertRaisesRegex(ValueError, 'Wrong trial asset'):
            self.verify(archive)

    def test_existing_package_preserved(self):
        path = self.root / 'existing'
        path.write_bytes(b'preserve me')
        with self.assertRaisesRegex(ValueError, 'Preserve'):
            p.build(path)
        self.assertEqual(path.read_bytes(), b'preserve me')

    def test_source_symlink_rejected(self):
        source = self.root / 'source'
        source.write_bytes(b'fixture')
        link = self.root / 'link'
        link.symlink_to(source)
        with self.assertRaises(OSError):
            p.read_source(link)

    def test_source_hardlink_rejected(self):
        source = self.root / 'source'
        source.write_bytes(b'fixture')
        os.link(source, self.root / 'link')
        with self.assertRaisesRegex(ValueError, 'Unsafe source'):
            p.read_source(source)

    def test_native_help_failure_is_not_bypassed(self):
        with patch.object(a, 'access_only', return_value={}), \
                patch.object(a.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, '', 'denied')):
            with self.assertRaisesRegex(ValueError, '--help failed'):
                a.user_gate(self.root, 'unused')

    def test_native_help_timeout_is_failure(self):
        with patch.object(a, 'access_only', return_value={}), \
                patch.object(a.subprocess, 'run', side_effect=subprocess.TimeoutExpired([], 3)):
            with self.assertRaises(subprocess.TimeoutExpired):
                a.user_gate(self.root, 'unused')

    def test_root_cannot_substitute_for_nonroot_gate(self):
        with patch.object(a.sys, 'platform', 'linux'), patch.object(a.os, 'geteuid', return_value=0):
            with self.assertRaisesRegex(ValueError, 'nonroot'):
                a.access_only(self.root, 'unused')

    def test_service_gate_cannot_run_on_mac(self):
        with patch.object(a.sys, 'platform', 'darwin'), patch.object(a.subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'Linux root'):
                a.service_gate(self.root, 'unused')
            run.assert_not_called()

    def test_service_gate_rejects_root_package_failure_before_unit_creation(self):
        with patch.object(a.sys, 'platform', 'linux'), patch.object(a.os, 'geteuid', return_value=0), \
                patch.object(a.os, 'uname', return_value=type('Uname', (), {'machine': 'x86_64'})()), \
                patch.object(a, 'check_files', side_effect=ValueError('bad permissions')), \
                patch.object(a.subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'bad permissions'):
                a.service_gate(self.root, 'unused')
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
