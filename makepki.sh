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

dir=pki
dbdir=nssdb
password=password

getchain() {
    local file="$1" cafiles=""
    shift 1

    while [ "`dirname $file`" != "$dir" ]; do
        file="`dirname $file`.crt"
        if [ -f "$file" ]; then
            cafiles="$cafiles -certfile \"$file\""
        fi
    done
    echo $cafiles
}

if [ ! -d "$dir" ]; then
    scriptdir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
    "$scriptdir"/makepki.py
fi

pwfile="$(mktemp)"
echo "$password" >"$pwfile"

if [ ! -d "$dbdir" ]; then
    mkdir "$dbdir"
    certutil -N -d "$dbdir" -f "$pwfile"
fi

find "$dir" -name \*.crt | while read certfile; do
    basename="${certfile%.crt}"
    keyfile="${basename}.key"
    crlfile="${basename}.crl"
    nick="${basename#${dir}/}"
    chain="`getchain $certfile`"
    p12file="${basename}.p12"

    echo "Creating $p12file from cert $cafile"
    export_command="openssl pkcs12 -export -out \"$p12file\" -in \"$certfile\"
                    -inkey \"$keyfile\" -name \"$nick\"
                    -passout pass:\"$password\" -passin file:\"$pwfile\"
                    $chain"

    eval $export_command
    pk12util -i "$p12file" -d "$dbdir" -k "$pwfile" -W "$password"

    if [ -f "$crlfile" ]; then
        certutil -d "$dbdir" -M -n "$nick" -t CT,C,C

        dercrlfile="$(mktemp)"

        openssl crl -in "$crlfile" -out "$dercrlfile" -outform DER
        crlutil -I -i "$dercrlfile" -d "$dbdir" -f "$pwfile" -B

        rm -f "$dercrlfile"
    fi
done

rm -f "$pwfile"
