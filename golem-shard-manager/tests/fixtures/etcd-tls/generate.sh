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
# Named curve: Go (etcd) rejects keys that spell out the curve parameters.
key() {
  openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 \
    -pkeyopt ec_param_enc:named_curve -out "$1" 2>/dev/null
}

# A version 3 CA: TLS libraries reject a version 1 certificate as an issuer.
ca() { # key, common name, output
  openssl req -new -key "$1" -subj "/CN=$2" -out "$3.csr"
  printf '%s\n' "basicConstraints=critical,CA:TRUE" "keyUsage=critical,keyCertSign,cRLSign" > "$3.ext"
  openssl x509 -req -in "$3.csr" -signkey "$1" -sha256 -days "$days" -extfile "$3.ext" \
    -out "$3" 2>/dev/null
  rm "$3.csr" "$3.ext"
}

key ca-key.pem
ca ca-key.pem golem-test-etcd-ca ca.pem

key other-ca-key.pem
ca other-ca-key.pem golem-test-other-ca other-ca.pem
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
