#!/usr/bin/env bash
# Regenerates the test PKI that the etcd TLS tests use. The certificates are valid for 100 years so
# that the committed files never expire under the tests.
#
#   ca.pem                          the CA that signs the etcd server and the shard manager client
#   server.pem / server-key.pem     etcd's server certificate: DNS localhost, IP 127.0.0.1
#   client.pem / client-key.pem     the shard manager's client certificate (mTLS)
#   other-ca.pem                    an unrelated CA, for the "wrong CA" test
set -euo pipefail
cd "$(dirname "$0")"

days=36500
key() { openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$1" 2>/dev/null; }

key ca-key.pem
openssl req -x509 -new -key ca-key.pem -sha256 -days "$days" -subj "/CN=golem-test-etcd-ca" \
  -out ca.pem

key other-ca-key.pem
openssl req -x509 -new -key other-ca-key.pem -sha256 -days "$days" -subj "/CN=golem-test-other-ca" \
  -out other-ca.pem
rm other-ca-key.pem

sign() { # name, common name, extensions
  key "$1-key.pem"
  openssl req -new -key "$1-key.pem" -subj "/CN=$2" -out "$1.csr"
  printf '%s\n' "$3" > "$1.ext"
  openssl x509 -req -in "$1.csr" -CA ca.pem -CAkey ca-key.pem -CAcreateserial -sha256 \
    -days "$days" -extfile "$1.ext" -out "$1.pem" 2>/dev/null
  rm "$1.csr" "$1.ext"
}

sign server localhost "$(printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nextendedKeyUsage=serverAuth')"
sign client golem-shard-manager "extendedKeyUsage=clientAuth"
# The tests never sign anything, so the CA key is not kept.
rm -f ca.srl ca-key.pem
