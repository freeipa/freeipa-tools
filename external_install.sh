#!/bin/bash
# Author: Stanislav Laznicka <slaznick@redhat.com>
# Description:
#       This simple bash script serves to simplify testing of FreeIPA
#       installations with an external CA

DBDIR="/tmp/ipa/ext_nssdb" # will be removed if exists!
PWDFILE="$DBDIR/pwdfile.txt"
NOISE="$DBDIR/noise.txt"
PASSWORD="Secret.123"

DOMAIN="bestdomain.ever"

if [ $EUID -ne 0 ]; then
   echo "This script must be run as root" 1>&2
   exit 1
fi


# Perform the first step of IPA installation, don't proceed if it fails
/usr/sbin/ipa-server-install -p ${PASSWORD} -a ${PASSWORD} \
                             --hostname $HOSTNAME --domain ${DOMAIN} \
                             -r ${DOMAIN^^} --external-ca \
                             --setup-dns --no-reverse --auto-forwarders -U \
|| exit $?

# Remove previous NSS database if it exists
if [ -e "$DBDIR" ]; then
    rm -rf "$DBDIR"
fi

# Get Subject Key Identifiers for the root and IPA CAs
ROOT_KEY_ID=0x$(dd if=/dev/urandom bs=20 count=1 | xxd -p)
IPA_CA_KEY_ID=0x$(dd if=/dev/urandom bs=20 count=1 | xxd -p)

# Prepare a new NSS database to serve us as an external CA
mkdir -p "$DBDIR"
echo "$PASSWORD" > "$PWDFILE"
# create noise file
dd count=10 bs=1024 if=/dev/random of="$NOISE" 2>/dev/null
certutil -N -d "$DBDIR" -f "$PWDFILE"

# Generate a CA certificate
echo -e "0\n1\n5\n6\n9\ny\ny\n\ny\n${ROOT_KEY_ID}\nn\n" \
    | certutil -d "$DBDIR" -S -s "CN=BEDNA,O=KRABICE" -n ca -t C,C,C -x \
     -1 -2 --extSKID -f "$PWDFILE" -z "$NOISE"

# Change the form of the CSR from PEM to DER for the NSS database
openssl req -outform der -in /root/ipa.csr -out "$DBDIR/req.csr"

# Sign the certificate request
echo -e "0\n1\n5\n6\n9\ny\ny\n\ny\ny\n${ROOT_KEY_ID}\
\n\n\nn\n${IPA_CA_KEY_ID}\nn\n" \
     | sudo certutil -C -d "$DBDIR" -m 1001 -i "$DBDIR/req.csr" \
       -o "$DBDIR/ipaca.cer" -c ca \
       -1 -2 -3 --extSKID -f "$PWDFILE" -z "$NOISE"

openssl x509 -inform der -in "$DBDIR/ipaca.cer" -out "$DBDIR/ipaca.pem"

# Export the NSS CA certificate and add it to a chain file
certutil -L -n ca -d "$DBDIR" -a > "$DBDIR/external.crt"
cat "$DBDIR/ipaca.pem" "$DBDIR/external.crt" > "$DBDIR/chain.crt"

/usr/sbin/ipa-server-install --external-cert-file "$DBDIR/chain.crt" -p ${PASSWORD}
