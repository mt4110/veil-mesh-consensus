#!/usr/bin/env python3
"""Explicit offline TLS provisioning (Python 3 + OpenSSL >= 3).

All outputs are new paths; nothing is overwritten or removed. Keep init-ca and
issue on a separate administrative host. Request/bundle run on the node; only
CSR and public certificates cross that boundary. This is a local fingerprint
revocation list, not an OCSP or X.509 CRL service.
"""
import argparse
import hashlib
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import sys

LIMIT = 16_384
NAME = re.compile(r'n[0-9a-f]{32}\.n[0-9a-f]{32}\.c[0-9a-f]{32}\.c[0-9a-f]{32}\.veil\.invalid\Z')


def openssl(*args, data=None):
    result = subprocess.run(['openssl', *map(str, args)], input=data, capture_output=True, timeout=15)
    if result.returncode:
        # Never echo stdout: genpkey/pkcs8 stdout can contain private key bytes.
        raise RuntimeError(f'openssl {args[0]} failed (exit {result.returncode})')
    return result.stdout


def read(path, secret=False, limit=LIMIT):
    path = Path(path)
    before = path.lstat()
    if not stat.S_ISREG(before.st_mode) or before.st_mode & 0o022:
        raise ValueError(f'input must be a non-writable regular file: {path}')
    if secret and stat.S_IMODE(before.st_mode) != 0o600:
        raise ValueError('private key must have mode 0600')
    with path.open('rb') as file:
        opened = os.fstat(file.fileno())
        if (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino):
            raise ValueError('input changed while opening')
        value = file.read(limit + 1)
    if len(value) > limit:
        raise ValueError('input exceeds size limit')
    return value


def write(path, value, mode=0o600):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    with os.fdopen(descriptor, 'wb') as file:
        file.write(value)
        file.flush()
        os.fsync(file.fileno())
    descriptor = os.open(Path(path).parent, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def key():
    pem = openssl('genpkey', '-algorithm', 'EC', '-pkeyopt', 'ec_paramgen_curve:P-256')
    return openssl('pkcs8', '-topk8', '-nocrypt', '-outform', 'DER', data=pem)


def validate_revoked(value):
    lines = value.decode('ascii').splitlines()
    if len(lines) > 4096 or len(set(lines)) != len(lines) or any(not re.fullmatch('[0-9a-f]{64}', line) for line in lines):
        raise ValueError('malformed or duplicate revoked certificate fingerprint')
    return set(lines)


def certificate_pem(der):
    return openssl('x509', '-inform', 'DER', '-outform', 'PEM', data=der)


def issue(ca_dir, csr, name, out, days):
    if not NAME.fullmatch(name) or not 1 <= days <= 90:
        raise ValueError('expected exact configuration-bound TLS name and 1..90 validity days')
    # Read/validate permissions before handing private-file paths to OpenSSL.
    read(ca_dir / 'ca.key.der', secret=True)
    ca = read(ca_dir / 'ca.cert.der')
    csr_bytes = read(csr)
    openssl('req', '-verify', '-inform', 'DER', '-noout', data=csr_bytes)
    openssl('x509', '-inform', 'DER', '-checkend', str(days * 86400 + 60), '-noout', data=ca)
    # The operator supplies the approved name. Do not copy CSR extensions.
    extensions = ('basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n'
                  'extendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:' + name + '\n')
    write(out / 'leaf.ext', extensions.encode())
    cert = openssl('x509', '-req', '-inform', 'DER', '-in', csr,
                   '-CA', ca_dir / 'ca.cert.der', '-CAform', 'DER',
                   '-CAkey', ca_dir / 'ca.key.der', '-CAkeyform', 'DER',
                   '-set_serial', '0x' + secrets.token_hex(16), '-days', days, '-sha256',
                   '-extfile', out / 'leaf.ext', '-outform', 'DER')
    # Confirm the issued certificate really verifies under the supplied root.
    verify_public_pair(out, ca, cert, name)
    write(out / 'ca.cert.der', ca)
    write(out / 'node.cert.der', cert)
    write(out / 'revoked.txt', b'')
    write(out / 'identity.txt', (name + '\nsha256=' + hashlib.sha256(cert).hexdigest() + '\n').encode())


def verify_public_pair(out, ca, cert, name):
    # Public-only verification artifacts retained in the issuance directory.
    write(out / 'verification-ca.pem', certificate_pem(ca))
    write(out / 'verification-node.pem', certificate_pem(cert))
    for purpose in ('sslclient', 'sslserver'):
        openssl('verify', '-CAfile', out / 'verification-ca.pem', '-no-CApath', '-no-CAstore',
                '-purpose', purpose, '-verify_hostname', name, out / 'verification-node.pem')


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='operation', required=True)
    init = sub.add_parser('init-ca'); init.add_argument('--out', type=Path, required=True); init.add_argument('--days', type=int, default=365)
    req = sub.add_parser('request'); req.add_argument('--out', type=Path, required=True)
    sign = sub.add_parser('issue'); sign.add_argument('--ca-dir', type=Path, required=True); sign.add_argument('--csr', type=Path, required=True); sign.add_argument('--tls-name', required=True); sign.add_argument('--out', type=Path, required=True); sign.add_argument('--days', type=int, default=30)
    bundle = sub.add_parser('bundle'); bundle.add_argument('--request-dir', type=Path, required=True); bundle.add_argument('--issued-dir', type=Path, required=True); bundle.add_argument('--revoked', type=Path, required=True); bundle.add_argument('--out', type=Path, required=True)
    revoke = sub.add_parser('revoke'); revoke.add_argument('--certificate', type=Path, required=True); revoke.add_argument('--previous', type=Path, required=True); revoke.add_argument('--out', type=Path, required=True)
    inspect = sub.add_parser('inspect'); inspect.add_argument('--certificate', type=Path, required=True)
    args = parser.parse_args()
    version = openssl('version').decode().strip()
    match = re.match(r'OpenSSL (\d+)\.', version)
    if not match or int(match[1]) < 3:
        raise ValueError('OpenSSL >= 3 is required')
    if args.operation == 'inspect':
        value = read(args.certificate)
        print(openssl('x509', '-inform', 'DER', '-noout', '-dates', '-subject', '-ext', 'subjectAltName', data=value).decode(), end='')
        print('sha256=' + hashlib.sha256(value).hexdigest())
        return
    if args.operation == 'revoke':
        cert = read(args.certificate)
        openssl('x509', '-inform', 'DER', '-noout', data=cert)
        previous = validate_revoked(read(args.previous, limit=65 * 4096))
        previous.add(hashlib.sha256(cert).hexdigest())
        value = ''.join(v + '\n' for v in sorted(previous)).encode()
        validate_revoked(value)
        write(args.out, value)
    else:
        args.out.mkdir(mode=0o700)  # exclusive generation directory
        if args.operation == 'init-ca':
            if not 2 <= args.days <= 3650:
                raise ValueError('CA validity must be 2..3650 days')
            write(args.out / 'ca.key.der', key())
            cert = openssl('req', '-new', '-x509', '-key', args.out / 'ca.key.der', '-keyform', 'DER',
                           '-subj', '/CN=Veil mesh offline CA', '-days', args.days, '-sha256',
                           '-set_serial', '0x' + secrets.token_hex(16), '-outform', 'DER',
                           '-addext', 'basicConstraints=critical,CA:TRUE,pathlen:0',
                           '-addext', 'keyUsage=critical,keyCertSign')
            write(args.out / 'ca.cert.der', cert)
        elif args.operation == 'request':
            write(args.out / 'node.key.der', key())
            csr = openssl('req', '-new', '-key', args.out / 'node.key.der', '-keyform', 'DER',
                          '-subj', '/CN=Veil mesh node', '-outform', 'DER')
            write(args.out / 'node.csr.der', csr)
        elif args.operation == 'issue':
            issue(args.ca_dir, args.csr, args.tls_name, args.out, args.days)
        elif args.operation == 'bundle':
            private = read(args.request_dir / 'node.key.der', secret=True)
            cert = read(args.issued_dir / 'node.cert.der')
            ca = read(args.issued_dir / 'ca.cert.der')
            revoked = read(args.revoked, limit=65 * 4096)
            deny = validate_revoked(revoked)
            if hashlib.sha256(cert).hexdigest() in deny:
                raise ValueError('cannot install a revoked certificate')
            pub_key = openssl('pkey', '-inform', 'DER', '-pubout', data=private)
            cert_key = openssl('x509', '-inform', 'DER', '-pubkey', '-noout', data=cert)
            if pub_key != cert_key:
                raise ValueError('certificate and private key do not match')
            for filename, value in [('node.key.der', private), ('node.cert.der', cert), ('ca.cert.der', ca), ('revoked.txt', revoked)]:
                write(args.out / filename, value)
    print(f'{args.operation}: wrote {args.out}; no existing files changed')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f'error: {error}', file=sys.stderr)
        sys.exit(1)
