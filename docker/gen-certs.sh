#!/bin/sh
# Generates a throwaway certificate authority and a server certificate for localhost into
# docker/certs/ (gitignored), for the TLS-enabled PostgreSQL in docker-compose.yml.
#
#   docker/certs/ca.crt       the CA: what a client trusts (TLS_CA_FILE in .env)
#   docker/certs/server.crt   the server's certificate, for DNS:localhost and IP:127.0.0.1
#   docker/certs/server.key   its private key
#
# Works with both OpenSSL and the LibreSSL that macOS ships. Rerun to replace the
# certificates (then `docker compose restart`).
set -eu
cd "$(dirname "$0")"
mkdir -p certs
cd certs

cat > ca.cnf <<'CNF'
[req]
distinguished_name = dn
[dn]
[v3_ca]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
CNF

cat > server.ext <<'EXT'
basicConstraints = CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = DNS:localhost, IP:127.0.0.1
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
EXT

openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -sha256 \
  -config ca.cnf -extensions v3_ca -subj "/CN=tokio-postgres-book test CA" \
  -keyout ca.key -out ca.crt 2>/dev/null
openssl req -newkey rsa:2048 -nodes -sha256 -config ca.cnf -subj "/CN=localhost" \
  -keyout server.key -out server.csr 2>/dev/null
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -days 825 -sha256 -extfile server.ext -out server.crt 2>/dev/null

rm -f server.csr server.ext ca.cnf ca.srl
chmod 600 ca.key server.key
echo "Wrote $(pwd)/ca.crt, server.crt, server.key"
