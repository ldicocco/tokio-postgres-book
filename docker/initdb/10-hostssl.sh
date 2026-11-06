#!/bin/sh
# Runs once, when the container initializes its data directory: from outside the container,
# accept TLS connections only, so `sslmode=disable` is refused (Chapter 12 shows the error).
set -eu
sed -i 's/^host all all all scram-sha-256$/hostssl all all all scram-sha-256/' "$PGDATA/pg_hba.conf"
grep -q '^hostssl all all all scram-sha-256$' "$PGDATA/pg_hba.conf"
