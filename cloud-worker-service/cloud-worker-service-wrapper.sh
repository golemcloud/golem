#!/bin/bash

# A wrapper script that's copied to Dockerfile as executable,
# which will renames the infra-specific
# configurations that are automatically uploaded (through terraform)
# in SSM, to something that shard-manager is specifically looking for.
# The right side values here are available through chamber that reads from SSM.
echo "Running in environment ${ENVIRONMENT}"
source <(chamber env golem-app/gateway-service/${ENVIRONMENT}) # FIXME
source <(chamber env golem-app/infra-outputs/${ENVIRONMENT})
# Custom names
export GOLEM__REDIS__HOST=$REDIS_HOST
export GOLEM__DOMAIN_RECORDS__DOMAIN_ALLOW_LIST="[$GOLEM__DOMAIN_RECORDS__DOMAIN_ALLOW_LIST]"
./cloud-worker-service