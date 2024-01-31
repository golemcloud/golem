#!/bin/bash

# A wrapper script that's copied to Dockerfile as executable,
# which will renames the infra-specific
# configurations that are automatically uploaded (through terraform)
# in SSM, to something that worker-executor specifically looking for.
# The right side values here are available through chamber that reads from SSM.
echo "Running in ${ENVIRONMENT}"
source <(chamber env golem-app/worker-executor/${ENVIRONMENT})
source <(chamber env golem-app/infra-outputs/${ENVIRONMENT})
export GOLEM__REDIS__HOST=$REDIS_HOST
export GOLEM__COMPILED_TEMPLATE_SERVICE__CONFIG__BUCKET=$COMPONENT_STORE_BUCKET_NAME
export GOLEM__TEMPLATE_SERVICE__CONFIG__ACCESS_TOKEN=$GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN # renamed
export GOLEM__RESOURCE_LIMITS__CONFIG__ACCESS_TOKEN=$GOLEM__RESOURCE_LIMITS__ACCESS_TOKEN # renamed

# This executable cloud-worker-executor is available only within in the docker context
./cloud-worker-executor
