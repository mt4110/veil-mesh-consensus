#!/usr/bin/env python3
"""Run on pi-node as UID 1000. Reuse its copied Nix runtime without a host mount.

--apply creates a fresh private workspace and one transient, unprivileged build
unit. No fstab/unit installation, host /nix changes, network download or reboot.
Existing source workspaces, build products, store and vendor data are read only.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

COPY = Path('/home/maakie/veil-arena-validation/run-1791624441124999000/results/kvm-runtime-copy/nix')
OLD = Path('/home/maakie/vlbsd-dohBzYcq')
VENDOR = Path('/home/maakie/veil-mesh-consensus/target/vendor')
CARGO = 'gmgfpzc0w2pgby0lmq9r88m39albiyfw-cargo-1.98.1/bin/cargo'
RUSTC = '53lvgpxsb3b9ghkgb4v0jra6zzlnyw8n-rustc-1.98.1/bin/rustc'

WORKER = r'''
import hashlib,json,os,pathlib,subprocess,sys,time
base=pathlib.Path(sys.argv[1]); host_namespace=int(sys.argv[2])
def require(ok,message):
 if not ok:raise RuntimeError(message)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
f=dict(v.split(':',1) for v in pathlib.Path('/proc/self/status').read_text().splitlines() if ':' in v)
mount=json.loads(subprocess.check_output(['findmnt','-J','-T','/nix','-o','TARGET,VFS-OPTIONS'],text=True))['filesystems'][0]
e=dict(uid=os.geteuid(),gid=os.getegid(),groups=os.getgroups(),caps={k:f[k].strip() for k in ['CapEff','CapPrm','CapInh','CapAmb','NoNewPrivs']},mount=mount,
 mount_namespace=os.stat('/proc/self/ns/mnt').st_ino,parent_namespace=host_namespace,complete=False)
try:
 require(os.geteuid()==os.getegid()==1000,'Build must be unprivileged')
 require(all(int(e['caps'][k],16)==0 for k in ['CapEff','CapPrm','CapInh','CapAmb']) and e['caps']['NoNewPrivs']=='1','Unexpected privileges')
 require(e['mount_namespace']!=host_namespace and mount['target']=='/nix' and 'ro' in mount['vfs-options'].split(','),'Store must be private/read-only')
 require(not pathlib.Path('/home/maakie/.ssh').exists() and not pathlib.Path('/run/docker.sock').exists(),'Host credentials/API must be hidden')
 manifest=json.loads((base/'source-manifest.json').read_text())
 env=dict(PATH='/usr/bin:/bin',HOME=str(base/'home'),CARGO_HOME=str(base/'cargo-home'),CARGO_TARGET_DIR=str(base/'target'),
          CARGO_INCREMENTAL='0',TMPDIR=str(base/'tmp'),XDG_CACHE_HOME=str(base/'cache'),LANG='C.UTF-8',RUSTC='/nix/store/'''+RUSTC+r'''')
 cargo='/nix/store/'''+CARGO+r''''
 e['versions']={name:subprocess.check_output([exe,'--version'],text=True,env=env).strip() for name,exe in [('cargo',cargo),('rustc',env['RUSTC'])]}
 commands=[('build',[cargo,'--config','source.crates-io.replace-with="vendored-sources"','--config','source.vendored-sources.directory="'''+str(VENDOR)+r'''"',
                     'build','--offline','--locked','--jobs','2','-p','veil-node','--bin','veil-local-ban']),
           ('regression',[cargo,'--config','source.crates-io.replace-with="vendored-sources"','--config','source.vendored-sources.directory="'''+str(VENDOR)+r'''"',
                          'test','--offline','--locked','--jobs','2','-p','veil-node','--lib'])]
 e['commands']=[]
 for label,command in commands:
  print('START '+label,flush=True); start=time.monotonic()
  with (base/(label+'.log')).open('x') as log:
   result=subprocess.run(command,cwd=base/'source',env=env,stdout=log,stderr=subprocess.STDOUT,timeout=900)
  e['commands'].append(dict(label=label,exit_code=result.returncode,elapsed_seconds=time.monotonic()-start))
  require(result.returncode==0,label+' failed; see retained log')
  print('PASS '+label,flush=True)
 require(all(sha(base/'source'/p)==h for p,h in manifest.items()),'Source changed during build')
 binary=base/'target/debug/veil-local-ban'
 e['binary']=dict(path=str(binary),sha256=sha(binary),size=binary.stat().st_size)
 e['source_unchanged']=True;e['complete']=True
except BaseException as error:
 e['error']=repr(error);raise
finally:
 with (base/'worker-result.json').open('x') as output:json.dump(e,output,indent=2)
'''


def require(ok, message):
    if not ok:
        raise RuntimeError(message)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def properties(base):
    return ['User=1000', 'Group=1000', 'PrivateMounts=yes', 'PrivateNetwork=yes', 'PrivateTmp=yes',
            'PrivateDevices=yes', 'ProtectSystem=strict', 'ProtectHome=tmpfs',
            f'BindPaths={base}', f'ReadWritePaths={base}',
            f'BindReadOnlyPaths={COPY}:/nix {VENDOR}', 'InaccessiblePaths=/run',
            'NoNewPrivileges=yes', 'CapabilityBoundingSet=', 'AmbientCapabilities=',
            'ProtectKernelTunables=yes', 'ProtectKernelModules=yes', 'ProtectKernelLogs=yes',
            'ProtectControlGroups=yes', 'ProtectClock=yes', 'RestrictSUIDSGID=yes',
            'MemoryMax=6G', 'TasksMax=256', 'Nice=10', 'IOWeight=50']


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--apply', action='store_true')
    args = p.parse_args()
    require(os.geteuid() == os.getegid() == 1000 and os.uname().machine == 'x86_64', 'Run as pi-node UID/GID 1000')
    for path in [COPY, COPY / 'store', COPY / 'store' / CARGO, COPY / 'store' / RUSTC, OLD, VENDOR]:
        require(path.exists() and not path.is_symlink(), 'Missing/unknown existing dependency: ' + str(path))
    facts = {str(p): dict(sha256=digest(p), inode=p.stat().st_ino, device=p.stat().st_dev, mode=p.stat().st_mode)
             for p in [COPY / 'store' / CARGO, COPY / 'store' / RUSTC]}
    if not args.apply:
        print(json.dumps(dict(existing_runtime=facts, host_store_visible=Path('/nix/store').exists()))); return
    os.umask(0o077)
    base = Path(tempfile.mkdtemp(prefix='vnr-', dir='/home/maakie'))
    for name in ['source', 'home', 'tmp', 'cargo-home', 'cache']:
        (base / name).mkdir(mode=0o700)
    paths = [OLD / p for p in ['Cargo.toml', 'Cargo.lock', 'veil-node/Cargo.toml', 'veil-common/Cargo.toml',
                               'xtask/Cargo.toml', 'veil-ebpf/Cargo.toml', 'assets/veil_xdp_kern.o']]
    for name in ['veil-node', 'veil-common', 'veil-ebpf', 'xtask']:
        paths += [p for p in (OLD / name / 'src').rglob('*') if p.suffix in ['.rs', '.c']]
    manifest = {}
    for path in paths:
        relative = path.relative_to(OLD)
        require(path.is_file() and not path.is_symlink() and path.resolve() == path, 'Unknown source path')
        target = base / 'source' / relative
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        with target.open('xb') as output:
            output.write(path.read_bytes())
        manifest[str(relative)] = digest(path)
    with (base / 'source-manifest.json').open('x') as output:
        json.dump(manifest, output, indent=2)
    with (base / 'worker.py').open('x') as output:
        output.write(WORKER)
    unit = 'veil-nix-build-' + base.name[4:] + '.service'
    state = subprocess.run(['systemctl', 'show', unit, '-p', 'LoadState', '--value'], capture_output=True, text=True, check=True)
    require(state.stdout.strip() == 'not-found', 'Preserve existing transient unit')
    command = ['sudo', '-n', 'systemd-run', '--unit=' + unit, '--wait', '--pipe', '--collect',
               '--service-type=exec', '--working-directory=' + str(base / 'source')]
    for value in properties(base):
        command += ['--property=' + value]
    command += ['/usr/bin/python3', '-I', '-B', '-u', str(base / 'worker.py'), str(base), str(os.stat('/proc/self/ns/mnt').st_ino)]
    with (base / 'run-manifest.json').open('x') as output:
        json.dump(dict(workspace=str(base), unit=unit, command=command, runtime_before=facts), output, indent=2)
    print('WORKSPACE=' + str(base), flush=True)
    print('TRANSIENT_UNIT=' + unit, flush=True)
    result = subprocess.run(command)
    require(all(digest(Path(path)) == entry['sha256'] and Path(path).stat().st_ino == entry['inode'] for path, entry in facts.items()), 'Existing runtime changed')
    require(all(digest(OLD / path) == h for path, h in manifest.items()), 'Existing workspace source changed')
    with (base / 'launcher-result.json').open('x') as output:
        json.dump(dict(exit_code=result.returncode, runtime_unchanged=True, old_source_unchanged=True), output, indent=2)
    raise SystemExit(result.returncode)


if __name__ == '__main__':
    main()
