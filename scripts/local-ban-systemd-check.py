#!/usr/bin/env python3
"""Transient systemd acceptance in the existing dedicated network namespace.
No account creation, /etc installation, firewall edits or deletion of evidence.
"""
import argparse, configparser, hashlib, http.client, json, os
from pathlib import Path
import pwd, signal, socket, ssl, struct, subprocess, sys, tempfile, time


def raw_request(directory, op=0, ttl=0, instance=None, revision=0, ip='10.99.0.1'):
    frame=struct.pack('>4sB3x16sQII',b'VLB1',op,bytes.fromhex(instance) if instance else bytes(16),revision,int.from_bytes(socket.inet_aton(ip),'big'),ttl)
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(2.5);s.connect(str(Path(directory)/'control.sock'));s.sendall(frame);s.shutdown(socket.SHUT_WR)
        data=b''
        while len(data)<=4096:
            chunk=s.recv(4097-len(data))
            if not chunk:break
            data+=chunk
    lines=data.decode().splitlines();assert lines and len(data)<=4096
    return dict(ok=lines[0]=='OK',**dict(x.split('=',1) for x in lines[1:] if '=' in x))


def main():
    p=argparse.ArgumentParser();p.add_argument('--binary',required=True);p.add_argument('--object',required=True)
    p.add_argument('--unit-template',required=True);p.add_argument('--artifacts-parent',required=True)
    p.add_argument('--test-user',default='nobody');args=p.parse_args();assert os.geteuid()==0
    os.umask(0o077);account=pwd.getpwnam(args.test_user);test_uid=account.pw_uid
    assert test_uid!=0 and account.pw_gid==test_uid
    assert str(test_uid) not in subprocess.check_output(['ps','-eo','uid='],text=True).split()
    root=Path(tempfile.mkdtemp(prefix='systemd-',dir=args.artifacts_parent))
    # Fresh root-owned public staging for ProtectHome; contains no credentials.
    stage=Path(tempfile.mkdtemp(prefix='veil-local-ban-test-',dir='/opt'));stage.chmod(0o755)
    runtime=Path(tempfile.mkdtemp(prefix='veil-local-ban-test-',dir='/var/lib'));runtime.chmod(0o755)
    binary=stage/'veil-local-ban';binary.write_bytes(Path(args.binary).read_bytes());binary.chmod(0o555)
    obj=stage/'veil_xdp_kern.o';obj.write_bytes(Path(args.object).read_bytes());obj.chmod(0o444)
    helper=stage/'client.py';helper.write_bytes(Path(__file__).read_bytes());helper.chmod(0o444)
    profile=configparser.ConfigParser(interpolation=None);profile.optionxform=str;profile.read(args.unit_template)
    names=[];servers=[];streams=[]
    results={'artifact_dir':str(root),'runtime_stage':str(runtime),'executable_stage':str(stage),'checks':[],
             'test_uid':test_uid,'account_is_existing_standin':True,'test_user':args.test_user,
             'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
             'object_sha256':hashlib.sha256(obj.read_bytes()).hexdigest()}
    def save():(root/'result.json').write_text(json.dumps(results,indent=2))
    def check(name,ok,**details):
        results['checks'].append(dict(name=name,passed=bool(ok),**details));save()
        print(('PASS ' if ok else 'FAIL ')+name,flush=True);assert ok,(name,details)
    def run(argv,**kwargs):return subprocess.run(argv,capture_output=True,text=True,timeout=kwargs.pop('timeout',20),**kwargs)
    def info(name):
        r=run(['systemctl','show',name,'--no-pager']);assert r.returncode==0,r.stderr
        return dict(x.split('=',1) for x in r.stdout.splitlines() if '=' in x)
    def link():return json.loads(subprocess.check_output(['ip','-j','-details','-n','ns-veil-rogue','link','show','veth-rogue']))[0]
    def attached():return bool(link().get('xdp',{}).get('attached'))
    def wait(fn,seconds=8):
        until=time.monotonic()+seconds
        while time.monotonic()<until:
            if fn():return True
            time.sleep(.05)
        return False
    def client(directory,op=0,ttl=0,instance=None,revision=0,ip='10.99.0.1'):
        request=json.dumps(dict(op=op,ttl=ttl,instance=instance,revision=revision,ip=ip))
        r=run(['setpriv',f'--reuid={test_uid}',f'--regid={test_uid}','--clear-groups',sys.executable,str(helper),'client',str(directory),request],timeout=5)
        assert r.returncode==0,r.stderr;return json.loads(r.stdout)
    def mutate(directory,op,ttl=0,ip='10.99.0.1'):
        state=client(directory);time.sleep(.21)
        return client(directory,op,ttl,state['instance'],int(state['revision']),ip)
    def directory(name):
        path=runtime/name;path.mkdir(mode=0o700);os.chown(path,test_uid,test_uid);return path
    def start(label,control,caps='CAP_BPF CAP_NET_ADMIN CAP_PERFMON',restart='on-abnormal',notify=True):
        name='veil-local-ban-test-'+stage.name.rsplit('-',1)[-1]+'-'+label+'.service';names.append(name)
        properties={k:v for k,v in profile['Service'].items() if k not in {'User','Group','ExecStart','EnvironmentFile','ReadWritePaths','StandardOutput','StandardError'}}
        properties.update(User=str(test_uid),Group=str(test_uid),ReadOnlyPaths=str(runtime),ReadWritePaths=str(control),
                          NetworkNamespacePath='/run/netns/ns-veil-rogue',CapabilityBoundingSet=caps,
                          AmbientCapabilities=caps,Restart=restart)
        properties.update({k:v for k,v in profile['Unit'].items() if k.startswith('StartLimit')})
        command=['systemd-run','--unit',name,'--service-type=notify','--quiet']
        for key,value in properties.items():command+=['--property',f'{key}={value}']
        command+=['--',str(binary),'serve']
        if notify:command+=['--systemd-notify']
        command+=['--iface','veth-rogue','--ebpf-object',str(obj),'--control-dir',str(control),'--destination','10.99.0.66','--port','18443','--protected-ip','198.51.100.7']
        (root/(label+'-'+control.name+'-command.json')).write_text(json.dumps(command,indent=2))
        r=run(command);(root/(label+'-'+control.name+'-start.txt')).write_text(r.stdout+r.stderr)
        wait(lambda:info(name)['ActiveState'] in ['active','failed'],8)
        return name,r
    def stop(name):
        r=run(['systemctl','stop',name]);assert r.returncode==0,r.stderr
        check(name+'_stopped',info(name)['ActiveState']=='inactive' and not attached())
    def capabilities(name):
        pid=int(info(name)['MainPID']);rows=[]
        for task in sorted(Path(f'/proc/{pid}/task').iterdir()):
            text=(task/'status').read_text();fields=dict(line.split(':',1) for line in text.splitlines() if ':' in line)
            rows.append({key:fields[key].strip() for key in ['Name','Uid','Groups','CapEff','CapPrm','CapInh','CapAmb','CapBnd','NoNewPrivs']})
        return rows
    def dump(name):
        (root/(name+'.show')).write_text(run(['systemctl','show',name]).stdout)
        (root/(name+'.journal')).write_text(run(['journalctl','--unit',name,'--no-pager','-n','150']).stdout)
    cert,key=root/'cert.pem',root/'test-key.pem'
    subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1','-subj','/CN=isolated-systemd','-addext','subjectAltName=IP:10.99.0.66','-keyout',str(key),'-out',str(cert)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    token=os.urandom(8).hex();context=ssl.create_default_context(cafile=str(cert))
    def https():
        try:
            c=http.client.HTTPSConnection('10.99.0.66',18443,context=context,timeout=1,source_address=('10.99.0.1',0))
            try:c.request('GET','/');r=c.getresponse();return r.status==200 and r.read()==token.encode()
            finally:c.close()
        except (OSError,http.client.HTTPException):return False
    try:
        results['link_before']=link();check('no_existing_xdp',not attached())
        f=open(root/'https.log','wb');streams.append(f)
        server=subprocess.Popen(['ip','netns','exec','ns-veil-rogue',sys.executable,str(Path(__file__).with_name('local-ban-check.py')),'server',str(cert),str(key),token],stdout=f,stderr=subprocess.STDOUT);servers.append(server)
        check('https_baseline',wait(https,5))
        control=directory('primary');name,r=start('main',control)
        primary_unit=name
        check('systemd_start_ready',r.returncode==0 and info(name)['ActiveState']=='active',output=r.stdout+r.stderr)
        check('ready_means_loaded_and_empty',attached() and client(control)['occupied']=='0')
        rows=capabilities(name);results['capabilities_after_start']=rows
        check('nonroot_capabilities_dropped_all_threads',all(int(row['Uid'].split()[0])==test_uid and all(int(g)==test_uid for g in row['Groups'].split()) and all(int(row[k],16)==0 for k in ['CapEff','CapPrm','CapInh','CapAmb']) and row['NoNewPrivs']=='1' for row in rows),threads=len(rows))
        check('ban_without_capabilities',mutate(control,1,3)['ok']);check('ban_drops_https',not https())
        check('kernel_ttl_restores_https',wait(https,5));check('cleanup_without_capabilities',wait(lambda:client(control)['occupied']=='0',4))
        check('explicit_ban',mutate(control,1,30)['ok']);check('explicit_unban',mutate(control,2)['ok']);check('unban_restores_https',https())
        old=client(control)['instance'];check('ban_before_restart',mutate(control,1,30)['ok'])
        rr=run(['systemctl','restart',name]);check('systemd_restart',rr.returncode==0 and info(name)['ActiveState']=='active')
        check('restart_clears_ban',client(control)['instance']!=old and client(control)['occupied']=='0' and https())
        for index in range(256):
            address=f'198.18.{index//256}.{index%256}'
            response=mutate(control,1,1800,ip=address)
            assert response['ok'],(index,response)
            if index%64==63:print(f'CAPACITY {index+1}/256',flush=True)
        check('service_capacity_256',client(control)['occupied']=='256')
        rejected=mutate(control,1,1800,ip='198.19.0.1')
        check('service_257_rejected',not rejected['ok'] and rejected.get('code')=='CAPACITY',response=rejected)
        check('full_keeps_256_and_normal_https',client(control)['occupied']=='256' and https())
        check('full_can_unban',mutate(control,2,ip='198.18.0.0')['ok'])
        old=client(control)['instance'];check('ban_before_sigkill',mutate(control,1,30)['ok'])
        oldpid=info(name)['MainPID'];rr=run(['systemctl','kill','--signal=SIGKILL','--kill-whom=main',name]);check('systemd_sigkill_sent',rr.returncode==0)
        check('abnormal_restart_ready',wait(lambda:info(name)['ActiveState']=='active' and info(name)['MainPID'] not in ['0',oldpid],8))
        check('abnormal_restart_empty',client(control)['instance']!=old and client(control)['occupied']=='0' and https())
        check('restart_count',int(info(name)['NRestarts'])>=1)
        results['capabilities_after_restart']=capabilities(name)
        check('restart_also_drops_capabilities',all(all(int(row[k],16)==0 for k in ['CapEff','CapPrm','CapInh','CapAmb']) for row in results['capabilities_after_restart']))
        results['primary_service']={k:info(name)[k] for k in ['User','Group','Restart','StartLimitBurst','StartLimitIntervalUSec','ReadWritePaths','CapabilityBoundingSet','MemoryMax','TasksMax']}
        dump(name);stop(name)
        check('stop_preserves_owner_record',sorted(x.name for x in control.iterdir())==['socket-owner'])
        proof=control/'socket-owner';proof.write_bytes(b'corrupted fixture: preserve')
        (control/'keep.txt').write_bytes(b'foreign data: keep exactly')
        before={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in control.iterdir()}
        # systemd unloads a stopped transient unit; recreate its exact service
        # profile instead of mistaking unit-not-found for application refusal.
        name,rr=start('corrupt',control)
        check('corruption_start_refused',info(name)['ActiveState']=='failed' and info(name)['ExecMainStatus']=='1')
        time.sleep(2.2)
        check('corruption_not_restart_loop',info(name)['Result']=='exit-code' and info(name)['NRestarts']=='0')
        check('corruption_data_preserved',{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in control.iterdir()}==before)
        dump(name)
        recovery=directory('recovery');other,rr=start('main',recovery)
        check('manual_recovery_same_unit_name',other==primary_unit)
        check('manual_recovery_service_ready',rr.returncode==0 and info(other)['ActiveState']=='active')
        check('manual_recovery_empty_https',client(recovery)['occupied']=='0' and https())
        check('manual_recovery_preserves_original',{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in control.iterdir()}==before)
        # Enter only this test service's mount namespace, then drop to its UID.
        # Opening without truncation must fail on the retained primary path.
        probe='import errno,os,sys\ntry:\n fd=os.open(sys.argv[1],os.O_WRONLY);os.close(fd)\nexcept OSError as e:\n sys.exit(0 if e.errno==errno.EROFS else 2)\nsys.exit(1)'
        rr=run(['nsenter','--target',info(other)['MainPID'],'--mount','--root','--wd=/','--','setpriv',f'--reuid={test_uid}',f'--regid={test_uid}','--clear-groups',sys.executable,'-c',probe,str(proof)])
        check('manual_recovery_primary_readonly',rr.returncode==0,returncode=rr.returncode,stderr=rr.stderr)
        dump(other);stop(other)
        for label,caps in [('no-bpf','CAP_NET_ADMIN CAP_PERFMON'),('no-netadmin','CAP_BPF CAP_PERFMON'),('no-perfmon','CAP_BPF CAP_NET_ADMIN')]:
            fresh=directory(label);unit,rr=start(label,fresh,caps=caps)
            check(label+'_fails_without_fallback',info(unit)['ActiveState']=='failed' and info(unit)['ExecMainStatus']=='1' and not attached())
            dump(unit)
        results['complete']=True
    except BaseException as error:
        results['complete']=False;results['error']=repr(error);raise
    finally:
        for name in names:
            dump(name);run(['systemctl','stop',name])
        for server in servers:
            if server.poll() is None:server.terminate();server.wait(timeout=5)
        for f in streams:f.close()
        results['link_after']=link();results['cleanup_ok']=not attached() and all(s.poll() is not None for s in servers)
        save();print('ARTIFACTS='+str(root),flush=True)


if __name__=='__main__':
    if len(sys.argv)>1 and sys.argv[1]=='client':
        print(json.dumps(raw_request(sys.argv[2],**json.loads(sys.argv[3]))))
    else:main()
