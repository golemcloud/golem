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

ADMIN_TOKEN="5c832d93-ff85-4a8f-9803-513950fdfdb1"

FS_BLOB_STORAGE_DIR="../local-run/data/blob_storage"

CLOUD_SERVICE_HTTP_PORT=8080
COMPONENT_COMPILATION_SERVICE_HTTP_PORT=8081
COMPONENT_SERVICE_HTTP_PORT=8082
SHARD_MANAGER_HTTP_PORT=8083
WORKER_EXECUTOR_HTTP_PORT=8084
WORKER_SERVICE_HTTP_PORT=8085
WORKER_SERVICE_CUSTOM_REQUEST_HTTP_PORT=9006
DEBUGGING_SERVICE_HTTP_PORT=8087

CLOUD_SERVICE_GRPC_PORT=9090
COMPONENT_COMPILATION_SERVICE_GRPC_PORT=9091
COMPONENT_SERVICE_GRPC_PORT=9092
SHARD_MANAGER_GRPC_PORT=9093
WORKER_EXECUTOR_GRPC_PORT=9094
WORKER_SERVICE_GRPC_PORT=9095

# start registry service
pushd golem-registry-service || exit

RUST_LOG=debug \
GOLEM__HTTP_PORT=${CLOUD_SERVICE_HTTP_PORT} \
GOLEM__GRPC_PORT=${CLOUD_SERVICE_GRPC_PORT} \
GOLEM__LOGIN__TYPE="${GOLEM_CLOUD_SERVICE_LOGIN_TYPE}" \
GOLEM__LOGIN__CONFIG__GITHUB__CLIENT_ID="${GITHUB_CLIENT_ID}" \
GOLEM__LOGIN__CONFIG__GITHUB__CLIENT_SECRET="${GITHUB_CLIENT_SECRET}" \
GOLEM__LOGIN__CONFIG__GITHUB__REDIRECT_URI="http://localhost:8080/v1/login/oauth2/web/callback" \
GOLEM__DB__TYPE="Sqlite" \
GOLEM__DB__CONFIG__DATABASE="../local-run/data/golem_registry_service.db" \
GOLEM__CORS_ORIGIN_REGEX="http://localhost:3000" \
GOLEM__ACCOUNTS__ROOT__TOKEN="${ADMIN_TOKEN}" \
../target/debug/golem-registry-service &

registy_service_pid=$!
popd || exit

# # start cloud service
# pushd cloud-service || exit

# RUST_LOG=info,h2=warn,hyper=warn,tower=warn \
# GOLEM__HTTP_PORT=${CLOUD_SERVICE_HTTP_PORT} \
# GOLEM__GRPC_PORT=${CLOUD_SERVICE_GRPC_PORT} \
# GOLEM__LOGIN__TYPE="${GOLEM_CLOUD_SERVICE_LOGIN_TYPE}" \
# GOLEM__LOGIN__CONFIG__GITHUB__CLIENT_ID="${GITHUB_CLIENT_ID}" \
# GOLEM__LOGIN__CONFIG__GITHUB__CLIENT_SECRET="${GITHUB_CLIENT_SECRET}" \
# GOLEM__LOGIN__CONFIG__GITHUB__REDIRECT_URI="http://localhost:9881/v1/login/oauth2/web/callback/github" \
# GOLEM__DB__TYPE="Sqlite" \
# GOLEM__DB__CONFIG__DATABASE="../local-run/data/golem_cloud_service.db" \
# GOLEM__CORS_ORIGIN_REGEX="http://localhost:3000" \
# GOLEM__ACCOUNTS__ROOT__TOKEN="${ADMIN_TOKEN}" \
# ../target/debug/cloud-service &

# cloud_service_pid=$!
# popd || exit

# # start component-compilation-service
# pushd golem-component-compilation-service || exit

# RUST_LOG=info,h2=warn,hyper=warn,tower=warn \
# GOLEM__HTTP_PORT=${COMPONENT_COMPILATION_SERVICE_HTTP_PORT} \
# GOLEM__GRPC_PORT=${COMPONENT_COMPILATION_SERVICE_GRPC_PORT} \
# GOLEM__BLOB_STORAGE__TYPE="LocalFileSystem" \
# GOLEM__BLOB_STORAGE__CONFIG__ROOT="${FS_BLOB_STORAGE_DIR}" \
# GOLEM__COMPONENT_SERVICE__CONFIG__PORT=${COMPONENT_SERVICE_GRPC_PORT} \
# GOLEM__COMPONENT_SERVICE__CONFIG__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# ../target/debug/golem-component-compilation-service &

# component_compilation_service_pid=$!
# popd || exit

# # start component-service
# pushd golem-component-service || exit

# RUST_LOG=info,h2=warn,hyper=warn,tower=warn \
# GOLEM__HTTP_PORT=${COMPONENT_SERVICE_HTTP_PORT} \
# GOLEM__GRPC_PORT=${COMPONENT_SERVICE_GRPC_PORT} \
# GOLEM__BLOB_STORAGE__TYPE="LocalFileSystem" \
# GOLEM__BLOB_STORAGE__CONFIG__ROOT="${FS_BLOB_STORAGE_DIR}" \
# GOLEM__CLOUD_SERVICE__PORT=${CLOUD_SERVICE_GRPC_PORT} \
# GOLEM__CLOUD_SERVICE__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__COMPILATION__TYPE="Enabled" \
# GOLEM__COMPILATION__CONFIG__PORT=${COMPONENT_COMPILATION_SERVICE_GRPC_PORT} \
# GOLEM__DB__TYPE="Sqlite" \
# GOLEM__DB__CONFIG__DATABASE="../local-run/data/golem_component.db" \
# GOLEM__CORS_ORIGIN_REGEX="http://localhost:3000" \
# ../target/debug/golem-component-service &

# component_service_pid=$!
# popd || exit

# # start shard-manager
# pushd golem-shard-manager || exit

# RUST_LOG=info,h2=warn,hyper=warn,tower=warn \
# GOLEM__HTTP_PORT=${SHARD_MANAGER_HTTP_PORT} \
# GOLEM__GRPC_PORT=${SHARD_MANAGER_GRPC_PORT} \
# GOLEM__PERSISTENCE__TYPE="FileSystem" \
# GOLEM__PERSISTENCE__CONFIG__PATH="../local-run/data/shard-manager/data.bin" \
# ../target/debug/golem-shard-manager &

# shard_manager_pid=$!
# popd || exit

## start worker-executor
# pushd golem-worker-executor || exit

# RUST_LOG=info \
# GOLEM__HTTP_PORT=${WORKER_EXECUTOR_HTTP_PORT} \
# GOLEM__PORT=${WORKER_EXECUTOR_GRPC_PORT} \
# GOLEM__PUBLIC_WORKER_API__PORT=${WORKER_SERVICE_GRPC_PORT} \
# GOLEM__PUBLIC_WORKER_API__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__BLOB_STORAGE__TYPE="LocalFileSystem" \
# GOLEM__BLOB_STORAGE__CONFIG__ROOT="${FS_BLOB_STORAGE_DIR}" \
# GOLEM__PLUGIN_SERVICE__CONFIG__PORT=${COMPONENT_SERVICE_GRPC_PORT} \
# GOLEM__PLUGIN_SERVICE__CONFIG__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__SHARD_MANAGER_SERVICE__CONFIG__PORT=${SHARD_MANAGER_GRPC_PORT} \
# GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_ATTEMPTS=10 \
# GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MIN_DELAY=1s \
# GOLEM__COMPONENT_SERVICE__CONFIG__PORT=${COMPONENT_SERVICE_GRPC_PORT} \
# GOLEM__COMPONENT_SERVICE__CONFIG__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__RESOURCE_LIMITS__CONFIG__PORT=${CLOUD_SERVICE_GRPC_PORT} \
# GOLEM__RESOURCE_LIMITS__CONFIG__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__PROJECT_SERVICE__CONFIG__PORT=${CLOUD_SERVICE_GRPC_PORT} \
# GOLEM__PROJECT_SERVICE__CONFIG__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# ../target/debug/worker-executor &

# worker_executor_pid=$!
# popd || exit

# # start worker-service
# pushd golem-worker-service || exit

# RUST_LOG=debug,h2=warn,hyper=warn,tower=warn \
# GOLEM__PORT=${WORKER_SERVICE_HTTP_PORT} \
# GOLEM__CUSTOM_REQUEST_PORT=${WORKER_SERVICE_CUSTOM_REQUEST_HTTP_PORT} \
# GOLEM__WORKER_GRPC_PORT=${WORKER_SERVICE_GRPC_PORT} \
# GOLEM__BLOB_STORAGE__TYPE="LocalFileSystem" \
# GOLEM__BLOB_STORAGE__CONFIG__ROOT="${FS_BLOB_STORAGE_DIR}" \
# GOLEM__DB__TYPE="Sqlite" \
# GOLEM__DB__CONFIG__DATABASE="../local-run/data/golem_worker.sqlite" \
# GOLEM__COMPONENT_SERVICE__PORT=${COMPONENT_SERVICE_GRPC_PORT} \
# GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__ROUTING_TABLE__PORT=${SHARD_MANAGER_GRPC_PORT} \
# GOLEM__CORS_ORIGIN_REGEX="http://localhost:3000" \
# GOLEM__CLOUD_SERVICE__PORT=${CLOUD_SERVICE_GRPC_PORT} \
# GOLEM__CLOUD_SERVICE__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# ../target/debug/golem-worker-service &

# worker_service_pid=$!
# popd || exit

# # start debugging service
# pushd golem-debugging-service || exit

# RUST_LOG=info \
# GOLEM__HTTP_PORT=${DEBUGGING_SERVICE_HTTP_PORT} \
# GOLEM__CLOUD_SERVICE__PORT=${CLOUD_SERVICE_GRPC_PORT} \
# GOLEM__CLOUD_SERVICE__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__PUBLIC_WORKER_API__PORT=${WORKER_SERVICE_GRPC_PORT} \
# GOLEM__PUBLIC_WORKER_API__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__BLOB_STORAGE__TYPE="LocalFileSystem" \
# GOLEM__BLOB_STORAGE__CONFIG__ROOT="${FS_BLOB_STORAGE_DIR}" \
# GOLEM__PLUGIN_SERVICE__CONFIG__PORT=${COMPONENT_SERVICE_GRPC_PORT} \
# GOLEM__PLUGIN_SERVICE__CONFIG__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__COMPONENT_SERVICE__PORT=${COMPONENT_SERVICE_GRPC_PORT} \
# GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__RESOURCE_LIMITS__CONFIG__PORT=${CLOUD_SERVICE_GRPC_PORT} \
# GOLEM__RESOURCE_LIMITS__CONFIG__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__PROJECT_SERVICE__CONFIG__PORT=${CLOUD_SERVICE_GRPC_PORT} \
# GOLEM__PROJECT_SERVICE__CONFIG__ACCESS_TOKEN="${ADMIN_TOKEN}" \
# GOLEM__CORS_ORIGIN_REGEX="http://localhost:3000" \
# ../target/debug/golem-debugging-service &

# debugging_service_pid=$!
# popd || exit

nginx -e /dev/stdout -p ./local-run -c ./nginx.conf &> ./local-run/logs/nginx.log &
router_pid=$!

echo "Started services"
echo " - cloud service:                 $registy_service_pid"
# echo " - worker executor:               $worker_executor_pid"
# echo " - worker service:                $worker_service_pid"
# echo " - component service:             $component_service_pid"
# echo " - component compilation service: $component_compilation_service_pid"
# echo " - shard manager:                 $shard_manager_pid"
# echo " - debugging service:             $debugging_service_pid"
echo " - router:                        $router_pid"
echo " - redis:                         $redis_pid"
echo ""
echo "Kill all manually:"
echo "kill -9 $cloud_service_pid $router_pid $redis_pid"

lnav ./local-run/logs

kill $cloud_service_pid || true
# kill $worker_executor_pid || true
# kill $worker_service_pid || true
# kill $component_service_pid || true
# kill $component_compilation_service_pid || true
# kill $shard_manager_pid || true
# kill $debugging_service_pid || true
kill $router_pid || true
kill $redis_pid || true
