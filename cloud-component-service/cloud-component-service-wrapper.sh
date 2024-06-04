#!/bin/bash

# A wrapper script that's copied to Dockerfile as executable,
# which will renames the infra-specific
# configurations that are automatically uploaded (through terraform)
# in SSM, to something that shard-manager is specifically looking for.
# The right side values here are available through chamber that reads from SSM.
echo "Running in environment ${ENVIRONMENT}"
source <(chamber env golem-app/component-service/${ENVIRONMENT})
source <(chamber env golem-app/infra-outputs/${ENVIRONMENT})
# Custom names
export GOLEM__DB__TYPE="Postgres"
export GOLEM__DB__CONFIG__HOST=$DB_HOST
export GOLEM__DB__CONFIG__DATABASE="golem_component_$ENVIRONMENT"
export GOLEM__DB__CONFIG__USERNAME=$DB_USERNAME
export GOLEM__DB__CONFIG__PASSWORD=$DB_PASSWORD
./cloud-component-service