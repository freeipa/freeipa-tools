#!/usr/bin/python3
#
# Copyright (c) 2015-2017, Jan Cholasta <jcholast@redhat.com>
#
# Permission to use, copy, modify, and/or distribute this software for any
# purpose with or without fee is hereby granted, provided that the above
# copyright notice and this permission notice appear in all copies.
#
# THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
# WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
# MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
# ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
# WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
# ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
# OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

import argparse
import base64
import collections
import datetime
import functools
import itertools
import os
import re
import sys

from cryptography import x509
from cryptography.hazmat.backends import default_backend
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID
from pyasn1.type import univ, char, namedtype, tag
from pyasn1.codec.der import encoder as der_encoder
from pyasn1.codec.native import decoder as native_decoder


def domain_name(s):
    if re.match(r'^\w[-\w]*(\.\w[-\w]*)*$', s):
        return s
    else:
        raise argparse.ArgumentTypeError("not a domain name")


def short_name(s):
    if re.match(r'^\w[-\w]*$', s):
        return s
    else:
        raise argparse.ArgumentTypeError("not a domain fragment")


parser = argparse.ArgumentParser(description="Make CA & service/client certs")
parser.add_argument('--domain', type=domain_name, default='example.com')
parser.add_argument(
    '--server', type=short_name, action='append', metavar='SHORTNAME',
    help='names of servers; can be given multiple times')
parser.add_argument(
    '--client', type=short_name, action='append', metavar='SHORTNAME',
    help='names of clients; can be given multiple times')
parser.add_argument(
    '--password', type=short_name, default='password',
    help='password for excrypting keys')

DIR = os.path.abspath('pki')

DAY = datetime.timedelta(days=1)
YEAR = 365 * DAY

CertInfo = collections.namedtuple('CertInfo', 'nick key cert counter')


# RFC 4120
class PrincipalName(univ.Sequence):
    componentType = namedtype.NamedTypes(
        namedtype.NamedType(
            'name-type',
            univ.Integer().subtype(
                explicitTag=tag.Tag(
                    tag.tagClassContext,
                    tag.tagFormatSimple,
                    0,
                ),
            ),
        ),
        namedtype.NamedType(
            'name-string',
            univ.SequenceOf(char.GeneralString()).subtype(
                explicitTag=tag.Tag(
                    tag.tagClassContext,
                    tag.tagFormatSimple,
                    1,
                ),
            ),
        ),
    )


# RFC 4556
class KRB5PrincipalName(univ.Sequence):
    componentType = namedtype.NamedTypes(
        namedtype.NamedType(
            'realm',
            char.GeneralString().subtype(
                explicitTag=tag.Tag(
                    tag.tagClassContext,
                    tag.tagFormatSimple,
                    0,
                ),
            ),
        ),
        namedtype.NamedType(
            'principalName',
            PrincipalName().subtype(
                explicitTag=tag.Tag(
                    tag.tagClassContext,
                    tag.tagFormatSimple,
                    1,
                ),
            ),
        ),
    )


def profile_ca(builder, ca_nick):
    now = datetime.datetime.utcnow()

    builder = builder.not_valid_before(now)
    builder = builder.not_valid_after(now + 10 * YEAR)

    crl_uri = 'file://{}.crl'.format(os.path.join(DIR, ca_nick))

    builder = builder.add_extension(
        x509.KeyUsage(
            digital_signature=True,
            content_commitment=True,
            key_encipherment=False,
            data_encipherment=False,
            key_agreement=False,
            key_cert_sign=True,
            crl_sign=True,
            encipher_only=False,
            decipher_only=False,
        ),
        critical=True,
    )
    builder = builder.add_extension(
        x509.BasicConstraints(ca=True, path_length=None),
        critical=True,
    )
    builder = builder.add_extension(
        x509.CRLDistributionPoints([
                x509.DistributionPoint(
                    full_name=[x509.UniformResourceIdentifier(crl_uri)],
                    relative_name=None,
                    crl_issuer=None,
                    reasons=None,
                ),
        ]),
        critical=False,
    )
    
    public_key = builder._public_key

    builder = builder.add_extension(
        x509.SubjectKeyIdentifier.from_public_key(public_key),
        critical=False,
    )

    return builder


def profile_server(builder, ca_nick,
                   warp=datetime.timedelta(days=0), dns_name=None,
                   badusage=False):
    now = datetime.datetime.utcnow() + warp

    builder = builder.not_valid_before(now)
    builder = builder.not_valid_after(now + YEAR)

    crl_uri = 'file://{}.crl'.format(os.path.join(DIR, ca_nick))

    builder = builder.add_extension(
        x509.CRLDistributionPoints([
                x509.DistributionPoint(
                    full_name=[x509.UniformResourceIdentifier(crl_uri)],
                    relative_name=None,
                    crl_issuer=None,
                    reasons=None,
                ),
        ]),
        critical=False,
    )

    if dns_name is not None:
        builder = builder.add_extension(
            x509.SubjectAlternativeName([x509.DNSName(dns_name)]),
            critical=False,
        )

    if badusage:
        builder = builder.add_extension(
            x509.KeyUsage(
                digital_signature=False,
                content_commitment=False,
                key_encipherment=False,
                data_encipherment=True,
                key_agreement=True,
                key_cert_sign=False,
                crl_sign=False,
                encipher_only=False,
                decipher_only=False
            ),
            critical=False
        )

    return builder


def profile_kdc(
        realm, builder, ca_nick,
        warp=datetime.timedelta(days=0), dns_name=None, badusage=False):
    """Needs to be curried with the realm."""
    now = datetime.datetime.utcnow() + warp

    builder = builder.not_valid_before(now)
    builder = builder.not_valid_after(now + YEAR)

    crl_uri = 'file://{}.crl'.format(os.path.join(DIR, ca_nick))

    builder = builder.add_extension(
        x509.ExtendedKeyUsage([x509.ObjectIdentifier('1.3.6.1.5.2.3.5')]),
        critical=False,
    )

    name = {
        'realm': realm,
        'principalName': {
            'name-type': 2,
            'name-string': ['krbtgt', realm],
        },
    }
    name = native_decoder.decode(name, asn1Spec=KRB5PrincipalName())
    name = der_encoder.encode(name)

    names = [x509.OtherName(x509.ObjectIdentifier('1.3.6.1.5.2.2'), name)]
    if dns_name is not None:
        names += [x509.DNSName(dns_name)]

    builder = builder.add_extension(
        x509.SubjectAlternativeName(names),
        critical=False,
    )

    builder = builder.add_extension(
        x509.CRLDistributionPoints([
                x509.DistributionPoint(
                    full_name=[x509.UniformResourceIdentifier(crl_uri)],
                    relative_name=None,
                    crl_issuer=None,
                    reasons=None,
                ),
        ]),
        critical=False,
    )

    if badusage:
        builder = builder.add_extension(
            x509.KeyUsage(
                digital_signature=False,
                content_commitment=False,
                key_encipherment=False,
                data_encipherment=True,
                key_agreement=True,
                key_cert_sign=False,
                crl_sign=False,
                encipher_only=False,
                decipher_only=False
            ),
            critical=False
        )

    return builder


def gen_cert(password, profile, nick_base, subject, ca=None, **kwargs):
    key = rsa.generate_private_key(
        public_exponent=65537,
        key_size=2048,
        backend=default_backend(),
    )
    public_key = key.public_key()

    counter = itertools.count(1)

    if ca is not None:
        ca_nick, ca_key, ca_cert, ca_counter = ca
        nick = os.path.join(ca_nick, nick_base)
        issuer = ca_cert.subject
    else:
        nick = ca_nick = nick_base
        ca_key = key
        ca_counter = counter
        issuer = subject

    serial = next(ca_counter)

    builder = x509.CertificateBuilder()
    builder = builder.serial_number(serial)
    builder = builder.issuer_name(issuer)
    builder = builder.subject_name(subject)
    builder = builder.public_key(public_key)

    builder = profile(builder, ca_nick, **kwargs)

    cert = builder.sign(
        private_key=ca_key,
        algorithm=hashes.SHA256(),
        backend=default_backend(),
    )

    key_pem = key.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.BestAvailableEncryption(password.encode()),
    )
    cert_pem = cert.public_bytes(serialization.Encoding.PEM)

    try:
        os.makedirs(os.path.dirname(os.path.join(DIR, nick)))
    except FileExistsError:
        pass
    with open(os.path.join(DIR, nick + '.key'), 'wb') as f:
        f.write(key_pem)
    with open(os.path.join(DIR, nick + '.crt'), 'wb') as f:
        f.write(cert_pem)

    return CertInfo(nick, key, cert, counter)


def revoke_cert(ca, serial):
    now = datetime.datetime.utcnow()

    crl_builder = x509.CertificateRevocationListBuilder()
    crl_builder = crl_builder.issuer_name(ca.cert.subject)
    crl_builder = crl_builder.last_update(now)
    crl_builder = crl_builder.next_update(now + DAY)

    crl_filename = os.path.join(DIR, ca.nick + '.crl')

    try:
        f = open(crl_filename, 'rb')
    except FileNotFoundError:
        pass
    else:
        with f:
            crl_pem = f.read()

        crl = x509.load_pem_x509_crl(crl_pem, default_backend())

        for revoked_cert in crl:
            crl_builder = crl_builder.add_revoked_certificate(revoked_cert)

    builder = x509.RevokedCertificateBuilder()
    builder = builder.serial_number(serial)
    builder = builder.revocation_date(now)

    revoked_cert = builder.build(default_backend())

    crl_builder = crl_builder.add_revoked_certificate(revoked_cert)

    crl = crl_builder.sign(
        private_key=ca.key,
        algorithm=hashes.SHA256(),
        backend=default_backend(),
    )

    crl_pem = crl.public_bytes(serialization.Encoding.PEM)

    with open(crl_filename, 'wb') as f:
        f.write(crl_pem)


def gen_server_certs(password, nick_base, hostname, org, ca=None):
    def make_dn(ou, cn_prefix):
        attrs = [
            x509.NameAttribute(NameOID.ORGANIZATION_NAME, org),
            x509.NameAttribute(NameOID.COMMON_NAME, cn_prefix + hostname),
        ]
        if ou is not None:
            attrs.insert(
                1, x509.NameAttribute(NameOID.ORGANIZATIONAL_UNIT_NAME, ou))
        return x509.Name(attrs)

    gen_cert(password, profile_server, nick_base, make_dn(None, ''), ca)
    gen_cert(
        password, profile_server, nick_base + '-badname',
        make_dn(None, 'bad-'), ca)
    gen_cert(
        password, profile_server, nick_base + '-altname',
        make_dn(None, 'alt-'), ca, dns_name=hostname)
    gen_cert(
        password, profile_server, nick_base + '-expired',
        make_dn('Expired', ''), ca, warp=-2 * YEAR)
    gen_cert(
        password, profile_server, nick_base + '-badusage',
        make_dn('Bad Usage', ''), ca, badusage=True)
    revoked = gen_cert(
        password, profile_server, nick_base + '-revoked',
        make_dn('Revoked', ''), ca)
    revoke_cert(ca, revoked.cert.serial_number)


def gen_kdc_certs(realm, password, nick_base, hostname, org, ca=None):
    _profile_kdc = functools.partial(profile_kdc, realm)

    def make_dn(ou, cn_prefix):
        return x509.Name([
            x509.NameAttribute(NameOID.ORGANIZATION_NAME, org),
            x509.NameAttribute(NameOID.ORGANIZATIONAL_UNIT_NAME, ou),
            x509.NameAttribute(NameOID.COMMON_NAME, cn_prefix + hostname),
        ])

    gen_cert(
        password, _profile_kdc, nick_base + '-kdc',
        make_dn('KDC', ''), ca)
    gen_cert(
        password, _profile_kdc, nick_base + '-kdc-badname',
        make_dn('KDC', 'not-'), ca)
    gen_cert(
        password, _profile_kdc, nick_base + '-kdc-altname',
        make_dn('KDC', 'alt-'), ca, dns_name=hostname)
    gen_cert(
        password, _profile_kdc, nick_base + '-kdc-expired',
        make_dn('Expired KDC', ''), ca, warp=-2 * YEAR)
    gen_cert(
        password, _profile_kdc, nick_base + '-kdc-badusage',
        make_dn('Bad Usage KDC', ''), ca, badusage=True)
    revoked = gen_cert(
        password, _profile_kdc, nick_base + '-kdc-revoked',
        make_dn('Revoked KDC', ''), ca)
    revoke_cert(ca, revoked.cert.serial_number)


def gen_subtree(args, nick_base, org, ca=None):
    def make_dn(cn):
        return x509.Name([
            x509.NameAttribute(NameOID.ORGANIZATION_NAME, org),
            x509.NameAttribute(NameOID.COMMON_NAME, cn),
        ])

    subca = gen_cert(args.password, profile_ca, nick_base, make_dn('CA'), ca)
    gen_cert(
        args.password, profile_server, 'wildcard',
        make_dn(f'*.{args.domain}'), subca)
    for shortname in set(args.server + args.client):
        cn = f'{shortname}.{args.domain}'
        gen_server_certs(args.password, shortname, cn, org, subca)
        gen_kdc_certs(
            args.domain.upper(), args.password, shortname, cn, org, subca)

    gen_cert(
        args.password, functools.partial(profile_kdc, args.domain.upper()),
        '-kdcwildcard', make_dn(f'*.{args.domain}'), subca)

    return subca


def main():
    args = parser.parse_args()
    if not args.server:
        args.server = ['server1', 'server2']
    if not args.client:
        args.client = ['client']

    for shortname in set(args.server):
        cn = f'{shortname}.{args.domain}'
        gen_cert(
            args.password, profile_server, f'{shortname}-selfsign',
            x509.Name([
                x509.NameAttribute(NameOID.ORGANIZATION_NAME, 'Self-signed'),
                x509.NameAttribute(NameOID.COMMON_NAME, cn),
            ])
        )
        gen_cert(
            args.password, functools.partial(profile_kdc, args.domain.upper()),
            f'{shortname}-kdc-selfsign',
            x509.Name([
                x509.NameAttribute(NameOID.ORGANIZATION_NAME, 'Self-signed'),
                x509.NameAttribute(NameOID.ORGANIZATIONAL_UNIT_NAME, 'KDC'),
                x509.NameAttribute(NameOID.COMMON_NAME, cn),
            ])
        )

    ca1 = gen_subtree(args, 'ca1', 'Example Organization')
    gen_subtree(args, 'subca', 'Subsidiary Example Organization', ca1)
    gen_subtree(args, 'ca2', 'Other Example Organization')
    ca3 = gen_subtree(args, 'ca3', 'Unknown Organization')
    os.unlink(os.path.join(DIR, ca3.nick + '.key'))
    os.unlink(os.path.join(DIR, ca3.nick + '.crt'))


if __name__ == '__main__':
    main()
