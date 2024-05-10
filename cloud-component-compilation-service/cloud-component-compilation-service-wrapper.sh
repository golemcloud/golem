#!/bin/bash

# A wrapper script that's copied to Dockerfile as executable,
# which will renames the infra-specific
# configurations that are automatically uploaded (through terraform)
# in SSM, to something that component-compilation-service is specifically looking for.
# The right side values here are available through chamber that reads from SSM.
echo "Running in environment ${ENVIRONMENT}"
source <(chamber env golem-app/worker-executor/${ENVIRONMENT})
source <(chamber env golem-app/infra-outputs/${ENVIRONMENT})
export GOLEM__COMPILED_COMPONENT_SERVICE__CONFIG__BUCKET=$COMPONENT_STORE_BUCKET_NAME
./cloud-component-compilation-service