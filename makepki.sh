#!/bin/bash -e
#
# Copyright (c) 2015, Jan Cholasta <jcholast@redhat.com>
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

domain=example.com
server1=server1.example.com
server2=server2.example.com
client=client.example.com
dbdir=nssdb
password=password

profile_ca_request_options=(-1 -2 -4)
profile_ca_request_input="\$'0\n1\n5\n6\n9\ny\ny\n\ny\n1\n7\nfile://'\$(readlink -f \$dbdir)/\$ca.crl\$'\n-1\n-1\n-1\nn\nn\n'"
profile_ca_create_options=(-v 120)
profile_ca_add_options=(-t CT,C,C)
profile_server_request_options=(-4)
profile_server_request_input="\$'1\n7\nfile://'\$(readlink -f \$dbdir)/\$ca.crl\$'\n-1\n-1\n-1\nn\nn\n'"
profile_server_create_options=(-v 12)
profile_server_add_options=(-t ,,)

gen_cert() {
    local profile="$1" nick="$2" subject="$3" ca request_options request_input create_options serial add_options pwfile noise csr crt
    shift 3

    ca="$(dirname $nick)"
    if [ "$ca" = "." ]; then
        ca="$nick"
    fi

    eval "request_options=(\"\${profile_${profile}_request_options[@]}\")"
    eval "eval request_input=\"\${profile_${profile}_request_input}\""

    eval "create_options=(\"\${profile_${profile}_create_options[@]}\")"
    if [ "$ca" = "$nick" ]; then
        create_options=("${create_options[@]}" -x -m 1)
    else
        eval "serial_${ca//\//_}=\$((\${serial_${ca//\//_}:-1}+1))"
        eval "serial=\$serial_${ca//\//_}"
        create_options=("${create_options[@]}" -c "$ca" -m "$serial")
    fi

    eval "add_options=(\"\${profile_${profile}_add_options[@]}\")"

    pwfile="$(mktemp)"
    echo "$password" >"$pwfile"

    noise="$(mktemp)"
    head -c 20 /dev/urandom >"$noise"

    if [ ! -d "$dbdir" ]; then
        mkdir "$dbdir"
        certutil -N -d "$dbdir" -f "$pwfile"
    fi

    csr="$(mktemp)"
    crt="$(mktemp)"
    certutil -R -d "$dbdir" -s "$subject" -f "$pwfile" -z "$noise" -o "$csr" "${request_options[@]}" >/dev/null <<<"$request_input"
    certutil -C -d "$dbdir" -f "$pwfile" -i "$csr" -o "$crt" "${create_options[@]}" "$@"
    certutil -A -d "$dbdir" -n "$nick" -f "$pwfile" -i "$crt" "${add_options[@]}"

    mkdir -p "$(dirname $dbdir/$nick.pem)"
    certutil -L -d "$dbdir" -n "$nick" -a >"$dbdir/$nick.pem"
    pk12util -o "$dbdir/$nick.p12" -n "$nick" -d "$dbdir" -k "$pwfile" -w "$pwfile"

    rm -f "$pwfile" "$noise" "$csr" "$crt"
}

revoke_cert() {
    local nick="$1" ca pwfile serial
    shift 1

    ca="$(dirname $nick)"
    if [ "$ca" = "." ]; then
        ca="$nick"
    fi

    pwfile="$(mktemp)"
    echo "$password" >"$pwfile"

    if ! crlutil -L -d "$dbdir" -n "$ca" &>/dev/null; then
        crlutil -G -d "$dbdir" -n "$ca" -c /dev/null -f "$pwfile"
    fi

    sleep 1

    mkdir -p "$(dirname $dbdir/$ca.crl)"
    serial=$(certutil -L -d "$dbdir" -n "$nick" | awk '/^\s+Serial Number: / { print $3 }')
    crlutil -M -d "$dbdir" -n "$ca" -c /dev/stdin -f "$pwfile" -o "$dbdir/$ca.crl" <<EOF
addcert $serial $(date -u +%Y%m%d%H%M%SZ)
EOF

    rm -f "$pwfile"
}

gen_server_certs() {
    local nick="$1" hostname="$2" org="$3"
    shift 3

    gen_cert server "$nick" "CN=$hostname,O=$org" "$@"
    gen_cert server "$nick-badname" "CN=not-$hostname,O=$org" "$@"
    gen_cert server "$nick-altname" "CN=alt-$hostname,O=$org" -8 "$hostname" "$@"
    gen_cert server "$nick-expired" "CN=$hostname,OU=Expired,O=$org" -w -24 "$@"
    gen_cert server "$nick-badusage" "CN=$hostname,OU=Bad Usage,O=$org" --keyUsage dataEncipherment,keyAgreement "$@"
    gen_cert server "$nick-revoked" "CN=$hostname,OU=Revoked,O=$org" "$@"
    revoke_cert "$nick-revoked"
}

gen_subtree() {
    local nick="$1" org="$2"
    shift 2

    gen_cert ca "$nick" "CN=CA,O=$org" "$@"
    gen_cert server "$nick/wildcard" "CN=*.$domain,O=$org"
    gen_server_certs "$nick/server" "$server1" "$org"
    gen_server_certs "$nick/replica" "$server2" "$org"
    gen_server_certs "$nick/client" "$client" "$org"
}

gen_cert server server-selfsign "CN=$server1,O=Self-signed"
gen_cert server replica-selfsign "CN=$server2,O=Self-signed"
gen_subtree ca1 'Example Organization'
gen_subtree ca1/subca 'Subsidiary Example Organization'
gen_subtree ca2 'Other Example Organization'
gen_subtree ca3 'Unknown Organization'
certutil -D -d "$dbdir" -n ca3
