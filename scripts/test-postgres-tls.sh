#!/usr/bin/env bash
#
# test-postgres-tls — run the hub's Postgres TLS tests (server/tests/postgres_tls.rs)
# against a throwaway local Postgres that accepts only TLS.
#
#   scripts/test-postgres-tls.sh
#
# Starts Postgres from a local initdb/pg_ctl (Homebrew `postgresql@16`, or apt
# `postgresql`) with a one-day CA and a server certificate valid only for
# "localhost", and an hba that accepts TLS connections only, so a successful
# connection proves TLS was used. Needs initdb, pg_ctl, createdb and openssl.
# Everything is removed afterwards.
#
# Portable to bash 3.2 (macOS default).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

for tool in initdb pg_ctl createdb openssl; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "test-postgres-tls: needs $tool (a local Postgres install and openssl)" >&2
    exit 2
  }
done

CLEANUP=""
cleanup() { if [ -n "$CLEANUP" ]; then eval "$CLEANUP" >/dev/null 2>&1 || true; fi; }
trap cleanup EXIT INT TERM

port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
# A short path: Postgres caps Unix socket paths at ~103 bytes, so sockets are
# disabled below and only TCP is used.
dir="$(mktemp -d /tmp/ctx-pgtls.XXXXXX)"
CLEANUP="rm -rf '$dir'"

openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=ctxone test CA" \
  -keyout "$dir/ca.key" -out "$dir/ca.crt" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" \
  -keyout "$dir/server.key" -out "$dir/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:localhost\n' >"$dir/san.ext"
openssl x509 -req -in "$dir/server.csr" -CA "$dir/ca.crt" -CAkey "$dir/ca.key" \
  -CAcreateserial -days 1 -extfile "$dir/san.ext" -out "$dir/server.crt" 2>/dev/null
chmod 600 "$dir/server.key"

initdb -D "$dir/data" -U ctx --auth=trust -E UTF8 >/dev/null
{
  echo "listen_addresses = 'localhost'"
  echo "port = $port"
  echo "unix_socket_directories = ''"
  echo "ssl = on"
  echo "ssl_cert_file = '$dir/server.crt'"
  echo "ssl_key_file = '$dir/server.key'"
} >>"$dir/data/postgresql.conf"
printf 'hostssl all all 127.0.0.1/32 trust\nhostssl all all ::1/128 trust\n' \
  >"$dir/data/pg_hba.conf"
LC_ALL="${LC_ALL:-C}" LANG="${LANG:-C}" \
  pg_ctl -D "$dir/data" -l "$dir/log" -w start >/dev/null
CLEANUP="pg_ctl -D '$dir/data' -m immediate stop; rm -rf '$dir'"
PGSSLMODE=require PGSSLROOTCERT="$dir/ca.crt" \
  createdb -h localhost -p "$port" -U ctx ctx_test

export TEST_DATABASE_TLS_URL="postgres://ctx@localhost:${port}/ctx_test"
export TEST_DATABASE_TLS_ROOTCERT="$dir/ca.crt"
echo "test-postgres-tls: TEST_DATABASE_TLS_URL=$TEST_DATABASE_TLS_URL"
cargo test -p ctxone-hub --test postgres_tls "$@"
