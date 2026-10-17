#!/bin/sh
# Test certificates for the shard manager client's TLS redirect test. The server certificate names
# only `localhost`, as a shard managers' certificate names only their Service, and no IP address.
set -eu
cd "$(dirname "$0")"
DAYS=36500

openssl ecparam -name prime256v1 -genkey -noout -out ca.key
openssl req -x509 -new -key ca.key -sha256 -days "$DAYS" -subj "/CN=golem test CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
  -out ca.crt

issue() {
  name=$1; ext=$2
  openssl ecparam -name prime256v1 -genkey -noout -out "$name.ec.key"
  openssl pkcs8 -topk8 -nocrypt -in "$name.ec.key" -out "$name.key"
  rm "$name.ec.key"
  openssl req -new -key "$name.key" -subj "/CN=$name" -out "$name.csr"
  printf '%s\n' "$ext" > "$name.ext"
  openssl x509 -req -in "$name.csr" -CA ca.crt -CAkey ca.key -CAcreateserial -sha256 \
    -days "$DAYS" -extfile "$name.ext" -out "$name.crt"
  rm "$name.csr" "$name.ext"
}

issue server "subjectAltName=DNS:localhost
extendedKeyUsage=serverAuth
basicConstraints=CA:FALSE"
issue client "extendedKeyUsage=clientAuth
basicConstraints=CA:FALSE"
rm -f ca.key ca.srl
