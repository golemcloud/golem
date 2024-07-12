#!/bin/bash

# A wrapper script that's copied to Dockerfile as executable,
# which will renames the infra-specific
# configurations that are automatically uploaded (through terraform)
# in SSM, to something that component-compilation-service is specifically looking for.
# The right side values here are available through chamber that reads from SSM.
source <(chamber env golem-app/worker-executor/${ENVIRONMENT})
source <(chamber env golem-app/infra-outputs/${ENVIRONMENT})
export GOLEM__BLOB_STORAGE__CONFIG__COMPILATION_CACHE_BUCKET=$COMPILED_COMPONENT_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__CUSTOM_DATA_BUCKET=$WORKER_CUSTOM_DATA_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__OPLOG_PAYLOAD_BUCKET=$WORKER_OPLOG_PAYLOAD_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__COMPRESSED_OPLOG_BUCKETS="[$WORKER_OPLOG_ARCHIVE_STORE_BUCKET_NAME]"



./cloud-component-compilation-service
