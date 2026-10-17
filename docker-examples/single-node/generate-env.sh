#!/usr/bin/env bash
# Prints the environment of the golemservices/golem image: new secrets, the S3 blob storage of every
# service and, when GOLEM_POSTGRES_HOST is set, the Postgres database of every store.
#
#   GOLEM_S3_BUCKET=my-bucket ./generate-env.sh > golem.env
#
# Inputs:
#   GOLEM_S3_BUCKET       the bucket of all blobs (required)
#   GOLEM_S3_REGION       the region of the bucket (default: us-east-1)
#   GOLEM_S3_ENDPOINT     the endpoint of an S3-compatible service (default: AWS S3)
#   GOLEM_S3_PATH_STYLE   true for path-style requests to the endpoint (default: unset)
#   GOLEM_POSTGRES_HOST, GOLEM_POSTGRES_PORT (default: 5432), GOLEM_POSTGRES_DB (default: golem),
#   GOLEM_POSTGRES_USER (default: golem), GOLEM_POSTGRES_PASSWORD
#                         the database; without GOLEM_POSTGRES_HOST, the state stays in SQLite
#
# Keep the output secret: it contains the admin token and the signing keys.
set -euo pipefail

: "${GOLEM_S3_BUCKET:?set GOLEM_S3_BUCKET to the bucket of all blobs}"
s3_region="${GOLEM_S3_REGION:-us-east-1}"
hmac_key="$(openssl rand -base64 32)"

echo "GOLEM__REGISTRY_SERVICE__INITIAL_ACCOUNTS__ROOT__TOKEN=$(openssl rand -hex 32)"
echo "GOLEM__WORKER_SERVICE__WEBHOOK_CALLBACK_HANDLER__HMAC_KEY=${hmac_key}"
echo "GOLEM__WORKER_EXECUTOR__AGENT_WEBHOOKS_SERVICE__HMAC_KEY=${hmac_key}"
echo "GOLEM__WORKER_SERVICE__INVOCATION_SESSION_TOKENS__ACTIVE_KEY__ID=key-1"
echo "GOLEM__WORKER_SERVICE__INVOCATION_SESSION_TOKENS__ACTIVE_KEY__KEY=$(openssl rand -base64 32)"

for service in REGISTRY_SERVICE WORKER_SERVICE WORKER_EXECUTOR COMPONENT_COMPILATION_SERVICE; do
  prefix="GOLEM__${service}__BLOB_STORAGE"
  echo "${prefix}__TYPE=S3"
  echo "${prefix}__CONFIG__REGION=${s3_region}"
  if [[ -n "${GOLEM_S3_ENDPOINT:-}" ]]; then
    echo "${prefix}__CONFIG__AWS_ENDPOINT_URL=${GOLEM_S3_ENDPOINT}"
  fi
  if [[ -n "${GOLEM_S3_PATH_STYLE:-}" ]]; then
    echo "${prefix}__CONFIG__AWS_PATH_STYLE=${GOLEM_S3_PATH_STYLE}"
  fi
  echo "${prefix}__CONFIG__OBJECT_PREFIX="
  echo "${prefix}__CONFIG__NAMESPACE_PREFIXES=true"
  for bucket in COMPILATION_CACHE CUSTOM_DATA OPLOG_PAYLOAD INITIAL_AGENT_FILES COMPONENTS FILESYSTEM_SNAPSHOTS; do
    echo "${prefix}__CONFIG__${bucket}_BUCKET=${GOLEM_S3_BUCKET}"
  done
  echo "${prefix}__CONFIG__COMPRESSED_OPLOG_BUCKETS=[\"${GOLEM_S3_BUCKET}\"]"
  echo "${prefix}__CONFIG__RETRIES__MAX_ATTEMPTS=3"
  echo "${prefix}__CONFIG__RETRIES__MIN_DELAY=100ms"
  echo "${prefix}__CONFIG__RETRIES__MAX_DELAY=1s"
  echo "${prefix}__CONFIG__RETRIES__MULTIPLIER=3.0"
  echo "${prefix}__CONFIG__RETRIES__MAX_JITTER_FACTOR=0.15"
done

if [[ -n "${GOLEM_POSTGRES_HOST:-}" ]]; then
  : "${GOLEM_POSTGRES_PASSWORD:?set GOLEM_POSTGRES_PASSWORD together with GOLEM_POSTGRES_HOST}"
  # The schemas of the per-service images, so the database can move to a fixed cluster.
  for store in \
    REGISTRY_SERVICE__DB:golem_registry_service \
    SHARD_MANAGER__PERSISTENCE:golem_shard_manager \
    WORKER_EXECUTOR__KEY_VALUE_STORAGE:golem_worker_executor \
    WORKER_EXECUTOR__INDEXED_STORAGE:golem_worker_executor_indexed \
    WORKER_EXECUTOR__SCHEDULER_STORAGE:golem_worker_executor_scheduler \
    WORKER_SERVICE__GATEWAY_SESSION_STORAGE:golem_worker_service_session_store; do
    prefix="GOLEM__${store%%:*}"
    echo "${prefix}__TYPE=Postgres"
    echo "${prefix}__CONFIG__HOST=${GOLEM_POSTGRES_HOST}"
    echo "${prefix}__CONFIG__PORT=${GOLEM_POSTGRES_PORT:-5432}"
    echo "${prefix}__CONFIG__DATABASE=${GOLEM_POSTGRES_DB:-golem}"
    echo "${prefix}__CONFIG__USERNAME=${GOLEM_POSTGRES_USER:-golem}"
    echo "${prefix}__CONFIG__PASSWORD=${GOLEM_POSTGRES_PASSWORD}"
    echo "${prefix}__CONFIG__MAX_CONNECTIONS=10"
    echo "${prefix}__CONFIG__SCHEMA=${store#*:}"
  done
  echo "GOLEM__WORKER_EXECUTOR__INDEXED_STORAGE__CONFIG__DROP_PREFIX_DELETE_BATCH_SIZE=1024"
  echo "GOLEM__WORKER_SERVICE__GATEWAY_SESSION_STORAGE__CONFIG__PENDING_LOGIN_EXPIRATION=1h"
  echo "GOLEM__WORKER_SERVICE__GATEWAY_SESSION_STORAGE__CONFIG__CLEANUP_INTERVAL=5m"
fi
