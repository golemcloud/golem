rm -rf ./local-run/data/shard-manager
mkdir -pv ./local-run/data/redis ./local-run/data/shard-manager ./local-run/logs

# start redis
redis-server --port 6380 --appendonly yes --dir ./local-run/data/redis &> ./local-run/logs/redis.log &
redis_pid=$!

export RUST_BACKTRACE=1

export GOLEM__TRACING__FILE_DIR="${GOLEM__TRACING__FILE_DIR:=../local-run/logs}"
export GOLEM__TRACING__FILE__ANSI="${GOLEM__TRACING__FILE__ANSI:=true}"
export GOLEM__TRACING__FILE__ENABLED="${GOLEM__TRACING__FILE__ENABLED:=true}"
export GOLEM__TRACING__FILE__JSON="${GOLEM__TRACING__FILE__JSON:=false}"
export GOLEM__TRACING__STDOUT__ENABLED="${GOLEM__TRACING__STDOUT__ENABLED:=false}"

ADMIN_TOKEN="lDL3DP2d7I3EbgfgJ9YEjVdEXNETpPkGYwyb36jgs28"

FS_BLOB_STORAGE_DIR="../local-run/data/blob_storage"

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
pushd golem-registry-service || exit

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
pushd golem-component-compilation-service || exit

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
pushd golem-shard-manager || exit

RUST_LOG=info,h2=warn,hyper=warn,tower=warn \
GOLEM__HTTP_PORT=${SHARD_MANAGER_HTTP_PORT} \
GOLEM__GRPC__PORT=${SHARD_MANAGER_GRPC_PORT} \
GOLEM__PERSISTENCE__TYPE="FileSystem" \
GOLEM__PERSISTENCE__CONFIG__PATH="../local-run/data/shard-manager/data.bin" \
../target/debug/golem-shard-manager &

shard_manager_pid=$!
popd || exit

# start worker-executor
pushd golem-worker-executor || exit

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
pushd golem-worker-service || exit

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

nginx -e /dev/stdout -p ./local-run -c ./nginx.conf &> ./local-run/logs/nginx.log &
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

lnav ./local-run/logs

kill $registy_service_pid || true
kill $worker_executor_pid || true
kill $worker_service_pid || true
kill $component_compilation_service_pid || true
kill $shard_manager_pid || true
kill $debugging_service_pid || true
kill $router_pid || true
kill $redis_pid || true
