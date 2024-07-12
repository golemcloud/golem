#!/bin/bash

# A wrapper script that's copied to Dockerfile as executable,
# which will renames the infra-specific
# configurations that are automatically uploaded (through terraform)
# in SSM, to something that shard-manager is specifically looking for.
# The right side values here are available through chamber that reads from SSM.
source <(chamber env golem-app/cloud-server/${ENVIRONMENT})
source <(chamber env golem-app/infra-outputs/${ENVIRONMENT})
# Custom names
export GOLEM__DB__TYPE="Postgres"
export GOLEM__DB__CONFIG__HOST=$DB_HOST
export GOLEM__DB__CONFIG__DATABASE="golem_$ENVIRONMENT"
export GOLEM__DB__CONFIG__USERNAME=$DB_USERNAME
export GOLEM__DB__CONFIG__PASSWORD=$DB_PASSWORD
export GOLEM__ACCOUNTS__ROOT__TOKEN=$INITIAL_ACCOUNT_TOKEN
export GOLEM__ACCOUNTS__MARKETING__TOKEN=$MARKETING_ACCOUNT_TOKEN
export GOLEM__ED_DSA__PRIVATE_KEY=$EDDSA_PRIVATE_KEY
export GOLEM__ED_DSA__PUBLIC_KEY=$EDDSA_PUBLIC_KEY
./cloud-service