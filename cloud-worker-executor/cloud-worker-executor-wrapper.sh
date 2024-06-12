#!/bin/bash

# A wrapper script that's copied to Dockerfile as executable,
# which will renames the infra-specific
# configurations that are automatically uploaded (through terraform)
# in SSM, to something that worker-executor specifically looking for.
# The right side values here are available through chamber that reads from SSM.
echo "Running in ${ENVIRONMENT}"
source <(chamber env golem-app/worker-executor/${ENVIRONMENT})
source <(chamber env golem-app/infra-outputs/${ENVIRONMENT})
export GOLEM__KEY_VALUE_STORAGE__CONFIG__HOST=$REDIS_HOST
export GOLEM__COMPONENT_SERVICE__CONFIG__ACCESS_TOKEN=$GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN # renamed
export GOLEM__RESOURCE_LIMITS__CONFIG__ACCESS_TOKEN=$GOLEM__RESOURCE_LIMITS__ACCESS_TOKEN    # renamed
export GOLEM__PUBLIC_WORKER_API__ACCESS_TOKEN=$GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN
export GOLEM__BLOB_STORAGE__CONFIG__COMPILATION_CACHE_BUCKET=$COMPILED_COMPONENT_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__CUSTOM_DATA_BUCKET=$WORKER_CUSTOM_DATA_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__OPLOG_PAYLOAD_BUCKET=$WORKER_OPLOG_PAYLOAD_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__COMPRESSED_OPLOG_BUCKETS="[$WORKER_OPLOG_ARCHIVE_STORE_BUCKET_NAME]"

# This executable cloud-worker-executor is available only within in the docker context
./cloud-worker-executor
