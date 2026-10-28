#!/usr/bin/env python3
"""Unprivileged, mocked safety checks; performs no network mutations."""
import importlib.util
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('namespace_helper', Path(__file__).with_name('local-ban-namespace.py'))
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


def existing():
    def link(name, mac, index, peer, address):
        return dict(ifname=name, address=mac, ifindex=index, link_index=peer, mtu=1500,
                    flags=['UP'], linkinfo={'info_kind': 'veth'},
                    addr_info=[dict(family='inet', local=address, prefixlen=24)])
    return dict(namespace=True, path_exists=True, path_safe=True,
                host=[link(helper.HOST, helper.HOST_MAC, 15, 14, '10.99.0.1')],
                peer=[dict(ifname='lo', addr_info=[]), link(helper.PEER, helper.PEER_MAC, 14, 15, '10.99.0.66')],
                routes=[dict(dst='10.99.0.0/24', dev=helper.HOST, prefsrc='10.99.0.1')],
                peer_routes=[dict(dst='10.99.0.0/24', dev=helper.PEER, prefsrc='10.99.0.66')])


def absent():
    return dict(namespace=False, path_exists=False, path_safe=False, host=[], routes=[], peer=[], peer_routes=[])


class ScopeTests(unittest.TestCase):
    def test_known_existing_and_absent(self):
        self.assertEqual(helper.classify(existing()), 'verified_existing')
        self.assertEqual(helper.classify(absent()), 'absent')

    def test_partial_or_unknown_namespace(self):
        for field, value in [('path_exists', False), ('path_safe', False), ('host', [])]:
            with self.subTest(field=field):
                state = existing(); state[field] = value
                with self.assertRaises(RuntimeError): helper.classify(state)
        state = absent(); state['path_exists'] = True
        with self.assertRaises(RuntimeError): helper.classify(state)
        state = absent(); state['host'] = existing()['host']
        with self.assertRaises(RuntimeError): helper.classify(state)

    def test_foreign_veth_state(self):
        for field, value in [('address', '00:00:00:00:00:01'), ('master', 'bridge'), ('flags', []),
                             ('ifalias', 'another-owner'), ('xdp', {'attached': 'generic'}),
                             ('mtu', 9000), ('link_index', 999)]:
            with self.subTest(field=field):
                state = existing(); state['host'][0][field] = value
                with self.assertRaises(RuntimeError): helper.classify(state)

    def test_unknown_namespace_interfaces_or_routes(self):
        state = existing(); state['peer'].append(dict(ifname='foreign'))
        with self.assertRaises(RuntimeError): helper.classify(state)
        state = existing(); state['peer_routes'].append(dict(dst='default', gateway='10.99.0.1'))
        with self.assertRaises(RuntimeError): helper.classify(state)
        state = existing(); state['host'].append(dict(ifname=helper.PEER))
        with self.assertRaises(RuntimeError): helper.classify(state)

    def test_extra_ip_or_foreign_subnet(self):
        state = existing(); state['host'][0]['addr_info'].append(dict(family='inet', local='10.99.0.2', prefixlen=24))
        with self.assertRaises(RuntimeError): helper.classify(state)
        state = absent(); state['host'] = [dict(ifname='other', addr_info=[dict(family='inet', local='10.99.0.2', prefixlen=24)])]
        with self.assertRaises(RuntimeError): helper.classify(state)

    def test_route_conflicts(self):
        for route in [dict(dst='10.0.0.0/8', dev='other'), dict(dst='10.99.0.0/24', dev='other'),
                      dict(dst='10.99.0.1/32', dev=helper.HOST, table='custom')]:
            with self.subTest(route=route):
                state = absent(); state['routes'] = [route]
                with self.assertRaises(RuntimeError): helper.classify(state)
        state = existing(); state['routes'].append(dict(dst='10.99.0.128/25', dev=helper.HOST))
        with self.assertRaises(RuntimeError): helper.classify(state)

    def test_live_namespace_or_global_ipv6_is_not_adopted(self):
        state = existing(); state['pids'] = [123]
        with self.assertRaises(RuntimeError): helper.classify(state)
        state = existing(); state['host'][0]['addr_info'].append(dict(family='inet6', local='2001:db8::1', prefixlen=64))
        with self.assertRaises(RuntimeError): helper.classify(state)

    def test_existing_restoration_never_mutates(self):
        with patch.object(helper, 'collect', return_value=existing()), patch.object(helper, 'run') as run:
            self.assertEqual(helper.restore(), dict(result='verified_existing', changed=False))
            run.assert_not_called()

    def test_absent_creation_is_finite_and_add_only(self):
        with patch.object(helper, 'collect', side_effect=[absent(), existing()]), patch.object(helper, 'run') as run:
            self.assertEqual(helper.restore(), dict(result='restored', changed=True))
            self.assertEqual(run.call_count, 7)
            for call in run.call_args_list:
                self.assertFalse({'delete', 'del', 'flush', 'replace', 'down'} & set(call.args))

    def test_unknown_state_never_mutates(self):
        state = existing(); state['path_safe'] = False
        with patch.object(helper, 'collect', return_value=state), patch.object(helper, 'run') as run:
            with self.assertRaises(RuntimeError): helper.restore()
            run.assert_not_called()

    def test_command_failure_has_no_automatic_cleanup_or_retry(self):
        with patch.object(helper, 'collect', return_value=absent()), patch.object(helper, 'run', side_effect=RuntimeError('failure')) as run:
            with self.assertRaises(RuntimeError): helper.restore()
            self.assertEqual(run.call_count, 1)

    def test_apply_is_explicit(self):
        with patch.object(sys, 'argv', ['helper', 'restore']), patch.object(helper, 'restore') as restore:
            with self.assertRaises(SystemExit) as raised: helper.main()
            self.assertEqual(raised.exception.code, 2)
            restore.assert_not_called()


if __name__ == '__main__':
    unittest.main()
