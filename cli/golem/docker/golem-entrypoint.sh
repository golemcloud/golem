#!/usr/bin/env bash
# Entrypoint of the golem image. Runs `golem` with the arguments of the container command.
#
# With GOLEM_LITESTREAM=true, Litestream replicates the SQLite databases of the data directory
# (GOLEM__DATA_DIR) to GOLEM_LITESTREAM_REPLICA_URL. Before the server starts, every database that
# the replica has and the data directory does not have is restored from the replica. Then the
# server runs as the child process of `litestream replicate`, which stops when the server stops.
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

data_dir="${GOLEM__DATA_DIR:-}"
[[ -n "${data_dir}" ]] || fail "GOLEM_LITESTREAM=true needs GOLEM__DATA_DIR"
replica_url="${GOLEM_LITESTREAM_REPLICA_URL:-}"
[[ -n "${replica_url}" ]] ||
  fail "GOLEM_LITESTREAM=true needs GOLEM_LITESTREAM_REPLICA_URL, for example s3://my-bucket/golem"

# The query of the URL, such as `?endpoint=...`, applies to every database, so the path of a
# database goes before it.
replica_base="${replica_url%%\?*}"
replica_base="${replica_base%/}"
replica_query=""
if [[ "${replica_url}" == *\?* ]]; then
  replica_query="?${replica_url#*\?}"
fi

# Litestream reads LITESTREAM_ACCESS_KEY_ID and LITESTREAM_SECRET_ACCESS_KEY as well, but s5cmd
# only reads the AWS variables.
if [[ -z "${AWS_ACCESS_KEY_ID:-}" && -n "${LITESTREAM_ACCESS_KEY_ID:-}" ]]; then
  export AWS_ACCESS_KEY_ID="${LITESTREAM_ACCESS_KEY_ID}"
fi
if [[ -z "${AWS_SECRET_ACCESS_KEY:-}" && -n "${LITESTREAM_SECRET_ACCESS_KEY:-}" ]]; then
  export AWS_SECRET_ACCESS_KEY="${LITESTREAM_SECRET_ACCESS_KEY}"
fi

query_param() {
  local name="$1" pair
  local IFS='&'
  for pair in ${replica_query#\?}; do
    if [[ "${pair%%=*}" == "${name}" ]]; then
      echo "${pair#*=}"
      return
    fi
  done
}

# Litestream reads LITESTREAM_S3_ENDPOINT for a replica URL on its command line, but not for one in
# its configuration file, so the endpoint goes into the query of the URL.
if [[ "${replica_base}" == s3://* && -n "${LITESTREAM_S3_ENDPOINT:-}" && -z "$(query_param endpoint)" ]]; then
  replica_query="${replica_query:-?}${replica_query:+&}endpoint=${LITESTREAM_S3_ENDPOINT}"
fi

# Prints the names of the databases that the replica has below the replica path `$1`: the
# replica of each database is a directory named like the database.
replica_databases() {
  local path="$1"
  case "${replica_base}" in
    file://*)
      local dir="${replica_base#file://}${path}"
      if [[ -d "${dir}" ]]; then
        find "${dir}" -mindepth 1 -maxdepth 1 -type d -name '*.db' -printf '%f\n'
      fi
      ;;
    s3://*)
      local s5cmd_args=() endpoint region output
      endpoint="$(query_param endpoint)"
      if [[ -n "${endpoint}" ]]; then
        [[ "${endpoint}" == *://* ]] || endpoint="https://${endpoint}"
        s5cmd_args+=(--endpoint-url "${endpoint}")
      fi
      region="$(query_param region)"
      if [[ -n "${region}" ]]; then
        export AWS_REGION="${region}"
      fi
      if ! output="$(s5cmd "${s5cmd_args[@]}" ls "${replica_base}${path}/" 2>&1)"; then
        if [[ "${output}" == *"no object found"* ]]; then
          return
        fi
        fail "Failed to list the Litestream replica ${replica_base}${path}/: ${output}"
      fi
      awk '$1 == "DIR" { sub("/$", "", $2); if ($2 ~ /\.db$/) print $2 }' <<<"${output}"
      ;;
    *)
      fail "GOLEM_LITESTREAM_REPLICA_URL must be an s3:// or a file:// URL, not '${replica_url}'"
      ;;
  esac
}

# Restores each database of the replica path `$1` that the local directory `$2` does not have.
restore_missing() {
  local path="$1" dir="$2" databases name
  databases="$(replica_databases "${path}")"
  while IFS= read -r name; do
    [[ -n "${name}" ]] || continue
    if [[ -e "${dir}/${name}" ]]; then
      continue
    fi
    log "Restoring ${dir}/${name} from ${replica_base}${path}/${name}"
    litestream restore -if-replica-exists -o "${dir}/${name}" \
      "${replica_base}${path}/${name}${replica_query}"
  done <<<"${databases}"
}

mkdir -p "${data_dir}/kv-store"
restore_missing "" "${data_dir}"
restore_missing "/kv-store" "${data_dir}/kv-store"

export GOLEM_LITESTREAM_DATA_DIR="${data_dir}"
export GOLEM_LITESTREAM_DATA_DIR_REPLICA_URL="${replica_base}${replica_query}"
export GOLEM_LITESTREAM_KV_STORE_REPLICA_URL="${replica_base}/kv-store${replica_query}"

# Litestream splits the command with shell quoting rules, so every argument is single-quoted.
command="golem"
for arg in "$@"; do
  command+=" '${arg//\'/\'\\\'\'}'"
done

exec litestream replicate -config /etc/golem/litestream.yml -exec "${command}"
