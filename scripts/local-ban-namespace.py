#!/usr/bin/env python3
"""Fixed pi-node test namespace: inspect, or restore an entirely absent layout.

No delete, flush, replace, partial repair, firewall, forwarding or default route
operations. Human approval is separate from --apply. Never run on another host.
"""
import argparse
import ipaddress
import json
import os
from pathlib import Path
import subprocess

IP = '/usr/sbin/ip'
NS = 'ns-veil-rogue'
HOST = 'veth-victim'
PEER = 'veth-rogue'
HOST_MAC = 'c2:75:ec:69:a9:d6'
PEER_MAC = 'c2:33:d1:70:88:00'
NETWORK = ipaddress.ip_network('10.99.0.0/24')


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def run(*args):
    p = subprocess.run([IP, *args], capture_output=True, text=True, timeout=3)
    require(p.returncode == 0, f'ip command failed: {args}: {p.stderr.strip()}')
    return p.stdout


def collect():
    names = [r['name'] for r in json.loads(run('-j', 'netns', 'list'))]
    path = Path('/run/netns') / NS
    present = NS in names
    return dict(
        namespace=present, path_exists=os.path.lexists(path),
        path_safe=present and not path.is_symlink() and os.path.ismount(path) and path.stat().st_uid == 0,
        host=json.loads(run('-j', '-details', 'address', 'show')),
        routes=json.loads(run('-j', '-4', 'route', 'show', 'table', 'all')),
        peer=json.loads(run('-j', '-details', '-n', NS, 'address', 'show')) if present else [],
        peer_routes=json.loads(run('-j', '-4', '-n', NS, 'route', 'show')) if present else [],
        pids=[int(v) for v in run('netns', 'pids', NS).split()] if present else [])


def ipv4(row):
    return {(a['local'], a['prefixlen']) for a in row.get('addr_info', []) if a['family'] == 'inet'}


def classify(state):
    host = {r['ifname']: r for r in state['host']}
    require(PEER not in host, 'Unexpected peer name in host namespace; preserve partial/foreign layout')
    present = state['namespace']
    require(present == state['path_exists'], 'Unknown namespace path; preserve it')
    if present:
        require(state['path_safe'], 'Unknown namespace mount; preserve it')
        require(not state.get('pids'), 'Namespace has running processes; preserve it')
        require(HOST in host, 'Partial layout; no automatic repair')
    else:
        require(HOST not in host, 'Existing host interface; no automatic adoption')
    for name, row in host.items():
        for address, _ in ipv4(row):
            require(ipaddress.ip_address(address) not in NETWORK or (present and name == HOST),
                    'Test subnet already used by another interface')
    for route in state['routes']:
        if route.get('dst', 'default') == 'default':
            continue
        if ipaddress.ip_network(route['dst'], strict=False).overlaps(NETWORK):
            require(present and route.get('dev') == HOST and not route.get('gateway')
                    and ((route.get('table', 'main') == 'main' and route['dst'] == str(NETWORK)
                          and route.get('type', 'unicast') == 'unicast')
                         or (route.get('table') == 'local' and
                             ((route.get('type') == 'local' and route['dst'] == '10.99.0.1')
                              or (route.get('type') == 'broadcast' and route['dst'] in ('10.99.0.0', '10.99.0.255'))))),
                    'Conflicting test subnet route; preserve it')
    if not present:
        require(not state['peer'] and not state['peer_routes'], 'Inconsistent absent namespace')
        return 'absent'
    peer = {r['ifname']: r for r in state['peer']}
    require(set(peer) == {'lo', PEER}, 'Unknown interfaces in test namespace')
    h, p = host[HOST], peer[PEER]
    for row, mac in [(h, HOST_MAC), (p, PEER_MAC)]:
        require(row.get('linkinfo', {}).get('info_kind') == 'veth' and not row.get('master')
                and row.get('address') == mac and row.get('mtu') == 1500
                and 'UP' in row['flags'] and not row.get('ifalias')
                and not row.get('xdp', {}).get('attached'), 'Unexpected veth identity/state/XDP')
        require(all(ipaddress.ip_address(a['local']).is_link_local
                    for a in row.get('addr_info', []) if a['family'] == 'inet6'),
                'Unexpected global IPv6 address on test interface')
    require(h.get('link_index') == p['ifindex'] and p.get('link_index') == h['ifindex'],
            'Peer identities do not match')
    require(ipv4(h) == {('10.99.0.1', 24)} and ipv4(p) == {('10.99.0.66', 24)}
            and not ipv4(peer['lo']), 'Unexpected IPv4 addresses')
    require(len(state['peer_routes']) == 1 and all(
        r.get('dst') == str(NETWORK) and r.get('dev') == PEER and not r.get('gateway')
        and r.get('prefsrc') == '10.99.0.66' for r in state['peer_routes']), 'Unexpected namespace route')
    require(any(r.get('dst') == str(NETWORK) and r.get('dev') == HOST
                and r.get('prefsrc') == '10.99.0.1' and r.get('table', 'main') == 'main'
                for r in state['routes']), 'Missing host test route')
    return 'verified_existing'


def restore():
    state = classify(collect())
    if state == 'verified_existing':
        return dict(result=state, changed=False)
    # Entirely absent only. A failure preserves partial state; do not retry or
    # delete/adopt it. A subsequent start must refuse that incomplete layout.
    commands = [
        ('netns', 'add', NS),
        ('link', 'add', 'name', HOST, 'address', HOST_MAC, 'mtu', '1500', 'type', 'veth',
         'peer', 'name', PEER, 'address', PEER_MAC),
        ('link', 'set', PEER, 'netns', NS),
        ('address', 'add', '10.99.0.1/24', 'dev', HOST),
        ('-n', NS, 'address', 'add', '10.99.0.66/24', 'dev', PEER),
        ('link', 'set', HOST, 'up'),
        ('-n', NS, 'link', 'set', PEER, 'up'),
    ]
    for command in commands:
        run(*command)
    require(classify(collect()) == 'verified_existing', 'Created layout failed verification')
    return dict(result='restored', changed=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['inspect', 'restore'])
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    if args.mode == 'restore' and not args.apply:
        parser.error('restore requires separate human approval and explicit --apply')
    require(os.geteuid() == 0, 'root is required for namespace inspection/restoration')
    result = restore() if args.mode == 'restore' else dict(result=classify(collect()), changed=False)
    print(json.dumps(result), flush=True)


if __name__ == '__main__':
    main()
