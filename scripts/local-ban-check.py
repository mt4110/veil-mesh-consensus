#!/usr/bin/env python3
"""Root-only isolated XDP/HTTPS acceptance; reuses the dedicated existing veth.
Does not change host firewall, clocks, physical interfaces, old WAL or keys.
Artifacts are retained. Only this run's children and owned runtime sockets close.
"""
import argparse, hashlib, http.client, http.server, ipaddress, json, os
from pathlib import Path
import signal, socket, ssl, struct, subprocess, sys, tempfile, threading, time


def serve_http(cert, key, token):
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = token.encode()
            self.send_response(200); self.send_header('Content-Length', str(len(body)))
            self.end_headers(); self.wfile.write(body)
        def log_message(self, *_): pass
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); context.load_cert_chain(cert, key)
    servers = []
    for port in (18443, 18444):
        server = http.server.ThreadingHTTPServer(('10.99.0.66', port), Handler)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start(); servers.append(server)
    signal.pause()


def main():
    p = argparse.ArgumentParser(); p.add_argument('--binary', required=True)
    p.add_argument('--object', required=True); p.add_argument('--artifacts-parent', required=True)
    args = p.parse_args(); assert os.geteuid() == 0, 'root required for dedicated namespace/XDP'
    os.umask(0o077)
    root = Path(tempfile.mkdtemp(prefix='local-ban-', dir=args.artifacts_parent))
    control = root / 'control'; control.mkdir(mode=0o700)
    ns = ['ip', 'netns', 'exec', 'ns-veil-rogue']
    evidence = {'artifact_dir': str(root), 'checks': [], 'binary_sha256': hashlib.sha256(Path(args.binary).read_bytes()).hexdigest(), 'object_sha256': hashlib.sha256(Path(args.object).read_bytes()).hexdigest()}
    daemon = server = None; logfiles = []; generation = 0; candidates = []
    owned_names = ('control.sock', 'emergency.sock', '.control.owned.sock', '.emergency.owned.sock')
    def check(name, condition, **detail):
        evidence['checks'].append(dict(name=name, passed=bool(condition), **detail))
        (root / 'result.json').write_text(json.dumps(evidence, indent=2))
        print(f'{"PASS" if condition else "FAIL"} {name}', flush=True)
        assert condition, (name, detail)
    def link():
        return json.loads(subprocess.check_output(['ip', '-j', '-details', '-n', 'ns-veil-rogue', 'link', 'show', 'veth-rogue']))[0]
    def attached(): return bool(link().get('xdp', {}).get('attached'))
    token = os.urandom(8).hex()
    cert, key = root / 'cert.pem', root / 'test-key.pem'
    subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=isolated-local-ban', '-addext', 'subjectAltName=IP:10.99.0.66', '-keyout', str(key), '-out', str(cert)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    ctx = ssl.create_default_context(cafile=str(cert))
    def https_probe(port=18443):
        try:
            c = http.client.HTTPSConnection('10.99.0.66', port, context=ctx, timeout=1.0, source_address=('10.99.0.1', 0))
            try:
                c.request('GET', '/'); r=c.getresponse(); return r.status == 200 and r.read() == token.encode()
            finally: c.close()
        except (OSError, http.client.HTTPException): return False
    def wait(fn, seconds=4):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            if fn(): return True
            time.sleep(.05)
        return False
    def request(op=0, ip='0.0.0.0', ttl=0, instance=None, revision=0, raw=None, emergency=False):
        frame = raw if raw is not None else struct.pack('>4sB3x16sQII', b'VLB1', op, bytes.fromhex(instance) if instance else bytes(16), revision, int(ipaddress.IPv4Address(ip)), ttl)
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(2.5); s.connect(str(control / ('emergency.sock' if emergency else 'control.sock')))
            s.sendall(frame); s.shutdown(socket.SHUT_WR); response=b''
            while len(response)<=4096:
                chunk=s.recv(4097-len(response))
                if not chunk: break
                response+=chunk
        lines=response.decode().splitlines(); assert lines and len(response)<=4096
        return dict(ok=lines[0]=='OK', **dict(line.split('=',1) for line in lines[1:] if '=' in line))
    def status(ip='10.99.0.1'): return request(ip=ip)
    def ready():
        try: return status().get('mode')=='local-ephemeral'
        except (OSError, AssertionError): return False
    def start():
        nonlocal daemon, generation
        generation+=1
        f=open(root/f'daemon-{generation}.log','wb'); logfiles.append(f)
        daemon=subprocess.Popen(ns+[args.binary,'serve','--iface','veth-rogue','--ebpf-object',args.object,'--control-dir',str(control),'--destination','10.99.0.66','--port','18443','--protected-ip','198.51.100.7'],stdout=f,stderr=subprocess.STDOUT)
        check(f'start_{generation}',wait(ready,8),returncode=daemon.poll())
        check(f'xdp_attached_{generation}',attached())
        return status()['instance']
    def stop(sig=signal.SIGTERM):
        nonlocal daemon
        if daemon and daemon.poll() is None:
            daemon.send_signal(signal.SIGCONT); daemon.send_signal(sig); daemon.wait(timeout=8)
        check(f'owned_link_detached_{generation}',not attached())
        check(f'owned_sockets_closed_{generation}',not any((control/name).exists() for name in owned_names))
        daemon=None
    def mutate(op, ip='10.99.0.1', ttl=0, expected=None, inst=None):
        s=status(); time.sleep(.21)
        return request(op,ip,ttl,inst or s['instance'],int(s['revision']) if expected is None else expected)
    try:
        evidence['link_before']=link(); check('no_existing_xdp',not attached())
        f=open(root/'https-server.log','wb'); logfiles.append(f)
        server=subprocess.Popen(ns+[sys.executable,__file__,'server',str(cert),str(key),token],stdout=f,stderr=subprocess.STDOUT)
        check('https_baseline',wait(https_probe,5)); check('other_port_baseline',https_probe(18444))
        instance=start(); check('empty_start',status()['occupied']=='0')
        proof=control/'socket-owner'
        check('fixed_ownership_record',proof.stat().st_size==72 and proof.stat().st_mode & 0o777 == 0o600)
        before_ownership={name:(control/name).stat().st_ino for name in owned_names}
        same=subprocess.run(ns+[args.binary,'serve','--iface','veth-rogue','--ebpf-object',args.object,'--control-dir',str(control),'--destination','10.99.0.66','--port','18443'],capture_output=True,text=True,timeout=5)
        check('same_directory_live_owner_preserved',same.returncode!=0 and 'CONTROL_DIRECTORY_BUSY' in same.stderr and before_ownership=={name:(control/name).stat().st_ino for name in owned_names} and status()['instance']==instance)
        foreign_dir=root/'foreign-control';foreign_dir.mkdir(mode=0o700)
        foreign_path=foreign_dir/'control.sock'
        with socket.socket(socket.AF_UNIX) as foreign:
            foreign.bind(str(foreign_path));foreign.listen(1);identity=foreign_path.stat().st_ino
            rejected=subprocess.run(ns+[args.binary,'serve','--iface','veth-rogue','--ebpf-object',args.object,'--control-dir',str(foreign_dir),'--destination','10.99.0.66','--port','18443'],capture_output=True,text=True,timeout=5)
            check('other_process_live_socket_preserved',rejected.returncode!=0 and foreign_path.stat().st_ino==identity and not (foreign_dir/'socket-owner').exists())
            with socket.socket(socket.AF_UNIX) as probe:
                probe.settimeout(1);probe.connect(str(foreign_path))
        rejected=subprocess.run(ns+[args.binary,'serve','--iface','veth-rogue','--ebpf-object',args.object,'--control-dir',str(foreign_dir),'--destination','10.99.0.66','--port','18443'],capture_output=True,text=True,timeout=5)
        check('unowned_dead_socket_preserved',rejected.returncode!=0 and foreign_path.stat().st_ino==identity)
        # A second owner must not replace an attached program/link.
        second_control=root/'second-control';second_control.mkdir(mode=0o700)
        before=link().get('xdp')
        collision=subprocess.run(ns+[args.binary,'serve','--iface','veth-rogue','--ebpf-object',args.object,'--control-dir',str(second_control),'--destination','10.99.0.66','--port','18443'],capture_output=True,text=True,timeout=5)
        (root/'collision.log').write_text(collision.stdout+collision.stderr)
        check('existing_xdp_preserved',collision.returncode!=0 and before==link().get('xdp'))
        # A pre-existing path is evidence, not something startup may unlink.
        blocked=root/'blocked-control';blocked.mkdir(mode=0o700)
        sentinel=blocked/'control.sock';sentinel.write_bytes(b'preserve-existing-file')
        refused=subprocess.run(ns+[args.binary,'serve','--iface','veth-rogue','--ebpf-object',args.object,'--control-dir',str(blocked),'--destination','10.99.0.66','--port','18443'],capture_output=True,text=True,timeout=5)
        check('existing_control_path_preserved',refused.returncode!=0 and sentinel.read_bytes()==b'preserve-existing-file' and before==link().get('xdp'))
        # CLI uses the same fixed protocol; preserve one human-readable example.
        cli=subprocess.run([args.binary,'status','--control-dir',str(control),'--ip','10.99.0.1'],capture_output=True,text=True,timeout=4)
        (root/'cli-status.txt').write_text(cli.stdout+cli.stderr); check('cli_status',cli.returncode==0)
        bad=request(raw=b'not-a-valid-frame'); check('short_frame_rejected',bad.get('code')=='INVALID_FRAME')
        bad=request(raw=struct.pack('>4sB3x16sQII',b'VLB1',0,bytes(16),0,0,0)+b'x'); check('trailing_bytes_rejected',bad.get('code')=='INVALID_FRAME')
        for ip in ['198.51.100.7','10.99.0.66','127.0.0.2']:
            r=mutate(1,ip,2); check('protected_'+ip,r.get('code')=='PROTECTED_OR_INVALID_IP')
        check('zero_ttl_rejected',mutate(1,ttl=0).get('code')=='INVALID_TTL')
        r=mutate(1,ttl=3); check('ban_applied',r.get('result')=='APPLIED',response=r)
        check('https_blocked',not https_probe()); check('other_port_passes_during_ban',https_probe(18444))
        check('kernel_drop_observed',int(status()['kernel_drop'])>0)
        daemon.send_signal(signal.SIGSTOP)
        time.sleep(3.1); check('kernel_expiry_while_daemon_stopped',https_probe())
        daemon.send_signal(signal.SIGCONT)
        check('bounded_cleanup_after_resume',wait(lambda:status().get('occupied')=='0',4))
        s=status(); check('cleanup_budget',int(s['max_examined'])<=16 and int(s['max_attempted'])<=16,stats=s)
        old_revision=int(s['revision']); r=mutate(1,ttl=30); check('second_ban',r['ok']); check('second_drop',not https_probe())
        check('explicit_unban',mutate(2)['ok']); check('https_after_unban',https_probe())
        r=request(1,'10.99.0.1',30,instance,old_revision); check('released_ban_replay_rejected',r.get('code')=='REVISION_CONFLICT'); check('replay_did_not_restore_ban',https_probe())
        # Saturate normal readers; emergency path must remain usable independently.
        check('ban_before_bypass',mutate(1,ttl=30)['ok']); check('drop_before_bypass',not https_probe())
        readers=[]
        for _ in range(8):
            x=socket.socket(socket.AF_UNIX);x.connect(str(control/'control.sock'));readers.append(x)
        time.sleep(.1)
        cs=request(emergency=True); r=request(3,instance=instance,revision=int(cs['control_revision']),emergency=True)
        check('emergency_with_eight_slow_readers',r.get('bypass')=='true');check('https_bypass_passes',https_probe())
        for x in readers:x.close()
        time.sleep(.1)
        cs=request(emergency=True);check('bypass_off',request(4,instance=instance,revision=int(cs['control_revision']),emergency=True)['ok'])
        check('ban_resumes_after_bypass_off',not https_probe())
        stop();check('https_after_stop',https_probe())
        second=start();check('restart_changes_instance',second!=instance);check('restart_clears_bans',status()['occupied']=='0' and https_probe())
        r=request(1,'10.99.0.1',30,instance,0);check('old_instance_rejected',r.get('code')=='INSTANCE_CONFLICT')
        # Fill at the production admission rate. No shortcuts, map resizing or TTL override.
        check('fill_first_target',mutate(1,ttl=180)['ok'])
        for n in range(255):
            ip=str(ipaddress.IPv4Address(int(ipaddress.IPv4Address('198.18.0.1'))+n))
            r=mutate(1,ip,180); assert r['ok'],(n,r)
        s=status();check('capacity_256',s['occupied']=='256' and s['active']=='256',stats=s)
        r=mutate(1,'198.19.1.1',180);check('new_ip_257_rejected',r.get('code')=='CAPACITY')
        check('existing_active_ban_not_evicted',not https_probe());check('other_port_at_capacity',https_probe(18444))
        check('event_ring_bounded',status()['event_count']=='64')
        evidence['status_at_capacity']=status()
        full_instance=status()['instance']
        before_kill={name:(control/name).stat().st_ino for name in owned_names}
        daemon.kill();daemon.wait(timeout=5)
        check('sigkill_reaped',daemon.returncode==-signal.SIGKILL)
        check('sigkill_owned_link_detached',not attached())
        check('sigkill_retains_owned_socket_paths',before_kill=={name:(control/name).stat().st_ino for name in owned_names})
        check('https_after_sigkill',https_probe())
        daemon=None
        # Race two recovery attempts. The directory lease must admit just one.
        generation+=1
        for n in range(2):
            f=open(root/f'daemon-recovery-{n}.log','wb');logfiles.append(f)
            candidates.append(subprocess.Popen(ns+[args.binary,'serve','--iface','veth-rogue','--ebpf-object',args.object,'--control-dir',str(control),'--destination','10.99.0.66','--port','18443','--protected-ip','198.51.100.7'],stdout=f,stderr=subprocess.STDOUT))
        check('concurrent_recovery_ready',wait(lambda:ready() and sum(c.poll() is None for c in candidates)==1,8))
        daemon=next(c for c in candidates if c.poll() is None)
        check('one_recovery_owner',sum(c.poll()==0 for c in candidates)==0 and sum(c.poll() is None for c in candidates)==1)
        s=status();check('restart_full_table_empty',s['occupied']=='0' and s['capacity']=='256' and https_probe())
        check('sigkill_restart_new_instance',s['instance']!=full_instance)
        check('prekill_command_rejected',request(1,'10.99.0.1',30,full_instance,256).get('code')=='INSTANCE_CONFLICT')
        check('postkill_ban_applied',mutate(1,ttl=30)['ok']);check('postkill_ban_blocks',not https_probe())
        check('postkill_unban_applied',mutate(2)['ok']);check('postkill_unban_restores_https',https_probe())
        stop()
        # A second abrupt cycle verifies the fixed paths do not accumulate history.
        fourth=start();check('repeated_sigkill_ban',mutate(1,ttl=30)['ok'])
        daemon.kill();daemon.wait(timeout=5);daemon=None
        check('second_sigkill_link_detached',not attached())
        fifth=start();check('repeated_sigkill_restart_empty',fifth!=fourth and status()['occupied']=='0' and https_probe());stop()
        check('no_policy_or_wal_files',sorted(p.name for p in control.iterdir())==['socket-owner'])
        check('ownership_record_stays_72_bytes',(control/'socket-owner').stat().st_size==72)
        evidence['complete']=True
    except BaseException as error:
        evidence['complete']=False; evidence['error']=repr(error); raise
    finally:
        if daemon and daemon.poll() is None:
            daemon.send_signal(signal.SIGCONT);daemon.send_signal(signal.SIGTERM)
            try:daemon.wait(timeout=8)
            except subprocess.TimeoutExpired:daemon.kill();daemon.wait(timeout=3)
        for c in candidates:
            if c.poll() is None:
                c.send_signal(signal.SIGCONT);c.terminate();c.wait(timeout=8)
        if server and server.poll() is None:server.terminate();server.wait(timeout=5)
        evidence['link_after']=link();evidence['cleanup_ok']=not attached() and (daemon is None or daemon.poll() is not None) and (server is None or server.poll() is not None) and all(c.poll() is not None for c in candidates)
        for f in logfiles:f.close()
        (root/'result.json').write_text(json.dumps(evidence,indent=2));print('ARTIFACTS='+str(root),flush=True)

if __name__=='__main__':
    if len(sys.argv)>1 and sys.argv[1]=='server':serve_http(*sys.argv[2:])
    else:main()
