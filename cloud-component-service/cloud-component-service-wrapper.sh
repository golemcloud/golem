#!/bin/bash

# A wrapper script that's copied to Dockerfile as executable,
# which will renames the infra-specific
# configurations that are automatically uploaded (through terraform)
# in SSM, to something that component-service is specifically looking for.
# The right side values here are available through chamber that reads from SSM.
source <(chamber env golem-app/component-service/${ENVIRONMENT})
source <(chamber env golem-app/infra-outputs/${ENVIRONMENT})
# Custom names
export GOLEM__DB__TYPE="Postgres"
export GOLEM__DB__CONFIG__HOST=$DB_HOST
export GOLEM__DB__CONFIG__DATABASE="golem_component_$ENVIRONMENT"
export GOLEM__DB__CONFIG__USERNAME=$DB_USERNAME
export GOLEM__DB__CONFIG__PASSWORD=$DB_PASSWORD

export GOLEM__BLOB_STORAGE__CONFIG__COMPILATION_CACHE_BUCKET=$COMPILED_COMPONENT_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__CUSTOM_DATA_BUCKET=$WORKER_CUSTOM_DATA_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__OPLOG_PAYLOAD_BUCKET=$WORKER_OPLOG_PAYLOAD_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__COMPRESSED_OPLOG_BUCKETS="[$WORKER_OPLOG_ARCHIVE_STORE_BUCKET_NAME]"
export GOLEM__BLOB_STORAGE__CONFIG__INITIAL_COMPONENT_FILES_BUCKET=$WORKER_INITIAL_COMPONENT_FILES_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__COMPONENTS_BUCKET=$COMPONENT_STORE_BUCKET_NAME
export GOLEM__BLOB_STORAGE__CONFIG__PLUGIN_WASM_FILES_BUCKET=$PLUGIN_WASM_FILES_BUCKET_NAME

./cloud-component-service
