#!/usr/bin/env bash
# Entrypoint of the golem image. Runs `golem` with the arguments of the container command.
#
# With GOLEM_LITESTREAM=true, the worker executor keeps its key-value storage and its indexed
# storage in one SQLite database each, instead of one database per agent. The server then has the
# fixed set of databases that /etc/golem/litestream.yml lists, and Litestream replicates each of
# them to GOLEM_LITESTREAM_REPLICA_URL. Before the server starts, every database of that list that
# the data directory (GOLEM__DATA_DIR) does not have is restored from the replica. Then the server
# runs as the child process of `litestream replicate`, which stops when the server stops.
set -euo pipefail

log() {
  echo "golem-entrypoint: $*" >&2
}

fail() {
  log "$*"
  exit 1
}

case "${GOLEM_LITESTREAM:-false}" in
  false) exec golem "$@" ;;
  true) ;;
  *) fail "GOLEM_LITESTREAM must be true or false, not '${GOLEM_LITESTREAM}'" ;;
esac

config=/etc/golem/litestream.yml

data_dir="${GOLEM__DATA_DIR:-}"
[[ -n "${data_dir}" ]] || fail "GOLEM_LITESTREAM=true needs GOLEM__DATA_DIR"
replica_url="${GOLEM_LITESTREAM_REPLICA_URL:-}"
[[ -n "${replica_url}" ]] ||
  fail "GOLEM_LITESTREAM=true needs GOLEM_LITESTREAM_REPLICA_URL, for example s3://my-bucket/golem"
case "${replica_url}" in
  s3://* | file://*) ;;
  *) fail "GOLEM_LITESTREAM_REPLICA_URL must be an s3:// or a file:// URL, not '${replica_url}'" ;;
esac

# The name of each database goes after the path of the URL, and the query, such as
# `?endpoint=...&region=...`, goes after that name unchanged. Litestream decodes the query. The
# first `?` of a URL starts its query.
replica_base="${replica_url%%\?*}"
export GOLEM_LITESTREAM_REPLICA_BASE="${replica_base%/}"
export GOLEM_LITESTREAM_REPLICA_QUERY=""
if [[ "${replica_url}" == *\?* ]]; then
  GOLEM_LITESTREAM_REPLICA_QUERY="?${replica_url#*\?}"
fi

export GOLEM__WORKER_EXECUTOR__KEY_VALUE_STORAGE__TYPE=Sqlite
export GOLEM__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__DATABASE="${data_dir}/key-value-storage.db"
export GOLEM__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__MAX_CONNECTIONS=4
export GOLEM__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__FOREIGN_KEYS=false
export GOLEM__WORKER_EXECUTOR__INDEXED_STORAGE__TYPE=Sqlite
export GOLEM__WORKER_EXECUTOR__INDEXED_STORAGE__CONFIG__DATABASE="${data_dir}/indexed-storage.db"
export GOLEM__WORKER_EXECUTOR__INDEXED_STORAGE__CONFIG__MAX_CONNECTIONS=4
export GOLEM__WORKER_EXECUTOR__INDEXED_STORAGE__CONFIG__FOREIGN_KEYS=false

# `litestream databases` prints a header, then the path and the replica type of each database.
databases="$(litestream databases -config "${config}")"
mkdir -p "${data_dir}"
[[ -w "${data_dir}" ]] ||
  fail "The data directory ${data_dir} is not writable by uid $(id -u); give it to uid 65534 and gid 65534"
while IFS= read -r database; do
  [[ -n "${database}" ]] || continue
  [[ -e "${database}" ]] || log "Restoring ${database} from the replica, if the replica has it"
  litestream restore -config "${config}" -if-db-not-exists -if-replica-exists "${database}"
done < <(sed -E -e '1d' -e 's/[[:space:]]+[^[:space:]]+$//' <<<"${databases}")

# Litestream splits the command with shell quoting rules, so every argument is single-quoted.
command="golem"
for arg in "$@"; do
  command+=" '${arg//\'/\'\\\'\'}'"
done

exec litestream replicate -config "${config}" -exec "${command}"
