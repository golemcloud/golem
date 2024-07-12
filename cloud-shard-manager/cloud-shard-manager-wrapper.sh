#!/bin/bash

# A wrapper script that's copied to Dockerfile as executable,
# which will renames the infra-specific
# configurations that are automatically uploaded (through terraform)
# in SSM, to something that shard-manager is specifically looking for.
# The right side values here are available through chamber that reads from SSM.
source <(chamber env golem-app/infra-outputs/${ENVIRONMENT})
# Custom names
export GOLEM__REDIS__HOST=$REDIS_HOST
export GOLEM__HEALTH_CHECK__MODE__CONFIG__NAMESPACE=$POD_NAMESPACE
./cloud-shard-manager