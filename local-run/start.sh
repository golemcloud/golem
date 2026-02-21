#!/usr/bin/env bash

# This script is sometimes executed via a temp "fsio" wrapper. In that case, `${BASH_SOURCE[0]}` points
# at a temp file. We therefore anchor paths based on the *current working directory* and common layouts.
START_PWD="$(pwd -P)"

# Try to find the `golem/` dir (the one that contains `local-run/`).
if [ -d "${START_PWD}/local-run" ] && [ -f "${START_PWD}/local-run/start.sh" ]; then
  GOLEM_DIR="${START_PWD}"
elif [ -d "${START_PWD}/golem/local-run" ] && [ -f "${START_PWD}/golem/local-run/start.sh" ]; then
  GOLEM_DIR="${START_PWD}/golem"
else
  # Fallback: assume we're already in the correct place (original behavior relied on cwd anyway).
  GOLEM_DIR="${START_PWD}"
fi

LOCAL_RUN_DIR="${GOLEM_DIR}/local-run"

rm -rf "${LOCAL_RUN_DIR}/data/shard-manager"
mkdir -pv "${LOCAL_RUN_DIR}/data/redis" "${LOCAL_RUN_DIR}/data/shard-manager" "${LOCAL_RUN_DIR}/logs"

# start redis
# Redis persistence isn't needed for local-run, and misconfigured snapshotting can force Redis into
# `stop-writes-on-bgsave-error` which then crashes worker-executor on startup.
#
# Also: if Redis is already running on 6380 (e.g. from a previous run), try to stop it first to avoid
# "Address already in use" aborts.
redis-cli -p 6380 ping >/dev/null 2>&1 && redis-cli -p 6380 shutdown nosave >/dev/null 2>&1 || true

redis-server \
  --port 6380 \
  --save "" \
  --appendonly no \
  --stop-writes-on-bgsave-error no \
  --dir "${LOCAL_RUN_DIR}/data/redis" &> "${LOCAL_RUN_DIR}/logs/redis.log" &
redis_pid=$!

# If Redis failed immediately (e.g. still port conflict), abort early with evidence.
if ! kill -0 "${redis_pid}" >/dev/null 2>&1; then
  echo "Redis failed to start on port 6380. Check ${LOCAL_RUN_DIR}/logs/redis.log"
  exit 1
fi

export RUST_BACKTRACE=1

export GOLEM__TRACING__FILE_DIR="${GOLEM__TRACING__FILE_DIR:=${LOCAL_RUN_DIR}/logs}"
export GOLEM__TRACING__FILE__ANSI="${GOLEM__TRACING__FILE__ANSI:=true}"
export GOLEM__TRACING__FILE__ENABLED="${GOLEM__TRACING__FILE__ENABLED:=true}"
export GOLEM__TRACING__FILE__JSON="${GOLEM__TRACING__FILE__JSON:=false}"
export GOLEM__TRACING__STDOUT__ENABLED="${GOLEM__TRACING__STDOUT__ENABLED:=false}"

ADMIN_TOKEN="lDL3DP2d7I3EbgfgJ9YEjVdEXNETpPkGYwyb36jgs28"

FS_BLOB_STORAGE_DIR="${LOCAL_RUN_DIR}/data/blob_storage"

REGISTRY_SERVICE_HTTP_PORT=8080
COMPONENT_COMPILATION_SERVICE_HTTP_PORT=8081
SHARD_MANAGER_HTTP_PORT=8082
WORKER_EXECUTOR_HTTP_PORT=8083
WORKER_SERVICE_HTTP_PORT=8084
DEBUGGING_SERVICE_HTTP_PORT=8085

REGISTRY_SERVICE_GRPC_PORT=9090
COMPONENT_COMPILATION_SERVICE_GRPC_PORT=9091
SHARD_MANAGER_GRPC_PORT=9092
WORKER_EXECUTOR_GRPC_PORT=9093
WORKER_SERVICE_GRPC_PORT=9094

WORKER_SERVICE_CUSTOM_REQUEST_HTTP_PORT=9005

# start registry service
pushd "${GOLEM_DIR}/golem-registry-service" || exit

RUST_LOG=debug \
GOLEM__HTTP_PORT=${REGISTRY_SERVICE_HTTP_PORT} \
GOLEM__GRPC__PORT=${REGISTRY_SERVICE_GRPC_PORT} \
GOLEM__LOGIN__TYPE="${GOLEM_REGISTRY_SERVICE_LOGIN_TYPE}" \
GOLEM__LOGIN__CONFIG__GITHUB__CLIENT_ID="${GITHUB_CLIENT_ID}" \
GOLEM__LOGIN__CONFIG__GITHUB__CLIENT_SECRET="${GITHUB_CLIENT_SECRET}" \
GOLEM__LOGIN__CONFIG__GITHUB__REDIRECT_URI="http://localhost:8080/v1/login/oauth2/web/callback" \
GOLEM__COMPILATION__TYPE="Enabled" \
GOLEM__COMPONENT_COMPILATION__CONFIG__HOST="localhost" \
GOLEM__COMPILATION__CONFIG__PORT=${COMPONENT_COMPILATION_SERVICE_GRPC_PORT} \
GOLEM__DB__TYPE="Sqlite" \
GOLEM__DB__CONFIG__DATABASE="../local-run/data/golem_registry_service.db" \
GOLEM__BLOB_STORAGE__TYPE="LocalFileSystem" \
GOLEM__BLOB_STORAGE__CONFIG__ROOT="${FS_BLOB_STORAGE_DIR}" \
GOLEM__CORS_ORIGIN_REGEX="http://localhost:3000" \
GOLEM__INITIAL_ACCOUNTS__ROOT__TOKEN="${ADMIN_TOKEN}" \
../target/debug/golem-registry-service &

registy_service_pid=$!
popd || exit

# start component-compilation-service
pushd "${GOLEM_DIR}/golem-component-compilation-service" || exit

RUST_LOG=info,h2=warn,hyper=warn,tower=warn \
GOLEM__HTTP_PORT=${COMPONENT_COMPILATION_SERVICE_HTTP_PORT} \
GOLEM__GRPC__PORT=${COMPONENT_COMPILATION_SERVICE_GRPC_PORT} \
GOLEM__BLOB_STORAGE__TYPE="LocalFileSystem" \
GOLEM__BLOB_STORAGE__CONFIG__ROOT="${FS_BLOB_STORAGE_DIR}" \
GOLEM__REGISTRY_SERVICE__CONFIG__HOST="localhost" \
GOLEM__REGISTRY_SERVICE__CONFIG__PORT=${REGISTRY_SERVICE_GRPC_PORT} \
../target/debug/golem-component-compilation-service &

component_compilation_service_pid=$!
popd || exit

# start shard-manager
pushd "${GOLEM_DIR}/golem-shard-manager" || exit

RUST_LOG=info,h2=warn,hyper=warn,tower=warn \
GOLEM__HTTP_PORT=${SHARD_MANAGER_HTTP_PORT} \
GOLEM__GRPC__PORT=${SHARD_MANAGER_GRPC_PORT} \
GOLEM__PERSISTENCE__TYPE="FileSystem" \
GOLEM__PERSISTENCE__CONFIG__PATH="../local-run/data/shard-manager/data.bin" \
../target/debug/golem-shard-manager &

shard_manager_pid=$!
popd || exit

# start worker-executor
pushd "${GOLEM_DIR}/golem-worker-executor" || exit

RUST_LOG=info \
GOLEM__HTTP_PORT=${WORKER_EXECUTOR_HTTP_PORT} \
GOLEM__GRPC__PORT=${WORKER_EXECUTOR_GRPC_PORT} \
GOLEM__PUBLIC_WORKER_API__PORT=${WORKER_SERVICE_GRPC_PORT} \
GOLEM__BLOB_STORAGE__TYPE="LocalFileSystem" \
GOLEM__BLOB_STORAGE__CONFIG__ROOT="${FS_BLOB_STORAGE_DIR}" \
GOLEM__REGISTRY_SERVICE__HOST="localhost" \
GOLEM__REGISTRY_SERVICE__PORT=${REGISTRY_SERVICE_GRPC_PORT} \
GOLEM__SHARD_MANAGER_SERVICE__CONFIG__PORT=${SHARD_MANAGER_GRPC_PORT} \
GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_ATTEMPTS=10 \
GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MIN_DELAY=1s \
../target/debug/worker-executor &

worker_executor_pid=$!
popd || exit

# start worker-service
pushd "${GOLEM_DIR}/golem-worker-service" || exit

RUST_LOG=debug,h2=warn,hyper=warn,tower=warn \
GOLEM__PORT=${WORKER_SERVICE_HTTP_PORT} \
GOLEM__CUSTOM_REQUEST_PORT=${WORKER_SERVICE_CUSTOM_REQUEST_HTTP_PORT} \
GOLEM__GRPC__PORT=${WORKER_SERVICE_GRPC_PORT} \
GOLEM__BLOB_STORAGE__TYPE="LocalFileSystem" \
GOLEM__BLOB_STORAGE__CONFIG__ROOT="${FS_BLOB_STORAGE_DIR}" \
GOLEM__DB__TYPE="Sqlite" \
GOLEM__DB__CONFIG__DATABASE="../local-run/data/golem_worker.sqlite" \
GOLEM__REGISTRY_SERVICE__HOST="localhost" \
GOLEM__REGISTRY_SERVICE__PORT=${REGISTRY_SERVICE_GRPC_PORT} \
GOLEM__ROUTING_TABLE__HOST="localhost" \
GOLEM__ROUTING_TABLE__PORT=${SHARD_MANAGER_GRPC_PORT} \
GOLEM__CORS_ORIGIN_REGEX="http://localhost:3000" \
../target/debug/golem-worker-service &

worker_service_pid=$!
popd || exit

# start debugging service
pushd golem-debugging-service || exit

RUST_LOG=info \
GOLEM__HTTP_PORT=${DEBUGGING_SERVICE_HTTP_PORT} \
GOLEM__PUBLIC_WORKER_API__PORT=${WORKER_SERVICE_GRPC_PORT} \
GOLEM__BLOB_STORAGE__TYPE="LocalFileSystem" \
GOLEM__BLOB_STORAGE__CONFIG__ROOT="${FS_BLOB_STORAGE_DIR}" \
GOLEM__REGISTRY_SERVICE__HOST="localhost" \
GOLEM__REGISTRY_SERVICE__PORT=${REGISTRY_SERVICE_GRPC_PORT} \
GOLEM__CORS_ORIGIN_REGEX="http://localhost:3000" \
../target/debug/golem-debugging-service &

debugging_service_pid=$!
popd || exit

nginx -e /dev/stdout -p "${LOCAL_RUN_DIR}" -c "${LOCAL_RUN_DIR}/nginx.conf" &> "${LOCAL_RUN_DIR}/logs/nginx.log" &
router_pid=$!

echo "Started services"
echo " - registry service: $registy_service_pid"
echo " - worker executor: $worker_executor_pid"
echo " - worker service: $worker_service_pid"
echo " - component compilation service: $component_compilation_service_pid"
echo " - shard manager: $shard_manager_pid"
echo " - debugging service:             $debugging_service_pid"
echo " - router: $router_pid"
echo " - redis: $redis_pid"
echo ""
echo "Kill all manually:"
echo "kill -9 $registy_service_pid $worker_executor_pid $worker_service_pid $component_compilation_service_pid $shard_manager_pid $router_pid $redis_pid"

lnav "${LOCAL_RUN_DIR}/logs"

kill $registy_service_pid || true
kill $worker_executor_pid || true
kill $worker_service_pid || true
kill $component_compilation_service_pid || true
kill $shard_manager_pid || true
kill $debugging_service_pid || true
kill $router_pid || true
kill $redis_pid || true
