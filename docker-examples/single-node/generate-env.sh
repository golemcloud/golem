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

echo "GOLEM_ADVANCED__REGISTRY_SERVICE__INITIAL_ACCOUNTS__ROOT__TOKEN=$(openssl rand -hex 32)"
echo "GOLEM_ADVANCED__WORKER_SERVICE__WEBHOOK_CALLBACK_HANDLER__HMAC_KEY=${hmac_key}"
echo "GOLEM_ADVANCED__WORKER_EXECUTOR__AGENT_WEBHOOKS_SERVICE__HMAC_KEY=${hmac_key}"
echo "GOLEM_ADVANCED__WORKER_SERVICE__INVOCATION_SESSION_TOKENS__ACTIVE_KEY__ID=key-1"
echo "GOLEM_ADVANCED__WORKER_SERVICE__INVOCATION_SESSION_TOKENS__ACTIVE_KEY__KEY=$(openssl rand -base64 32)"

echo "GOLEM__BLOB_STORE=s3"
echo "GOLEM__OBJECT_STORAGE__REGION=${s3_region}"
echo "GOLEM__OBJECT_STORAGE__COMPONENTS_BUCKET=${GOLEM_S3_BUCKET}"
echo "GOLEM__OBJECT_STORAGE__DATA_BUCKET=${GOLEM_S3_BUCKET}"
if [[ -n "${GOLEM_S3_ENDPOINT:-}" ]]; then
  echo "GOLEM__OBJECT_STORAGE__ENDPOINT_URL=${GOLEM_S3_ENDPOINT}"
fi
if [[ -n "${GOLEM_S3_PATH_STYLE:-}" ]]; then
  echo "GOLEM__OBJECT_STORAGE__PATH_STYLE=${GOLEM_S3_PATH_STYLE}"
fi

if [[ -n "${GOLEM_POSTGRES_HOST:-}" ]]; then
  : "${GOLEM_POSTGRES_PASSWORD:?set GOLEM_POSTGRES_PASSWORD together with GOLEM_POSTGRES_HOST}"
  echo "GOLEM__STATE_STORE=postgres"
  echo "GOLEM__POSTGRES__HOST=${GOLEM_POSTGRES_HOST}"
  echo "GOLEM__POSTGRES__PORT=${GOLEM_POSTGRES_PORT:-5432}"
  echo "GOLEM__POSTGRES__DATABASE=${GOLEM_POSTGRES_DB:-golem}"
  echo "GOLEM__POSTGRES__USERNAME=${GOLEM_POSTGRES_USER:-golem}"
  echo "GOLEM__POSTGRES__PASSWORD=${GOLEM_POSTGRES_PASSWORD}"
  echo "GOLEM__POSTGRES__MAX_CONNECTIONS=10"
fi
