#!/bin/bash
set -euo pipefail
IFS=$'\n\t'

ELASTICSEARCH_HOST="http://es01:9200"
KIBANA_HOST="http://kibana:5601"

until curl --output /dev/null --silent --head --fail $ELASTICSEARCH_HOST; do
    printf '.'
    sleep 1
done

until curl --output /dev/null --silent --head --fail $KIBANA_HOST; do
    printf '.'
    sleep 1
done

curl -X POST "$KIBANA_HOST/api/saved_objects/_import?createNewCopies=false" -H "kbn-xsrf: true" --form file=@/provision/saved_objects.ndjson -H 'kbn-xsrf: true'
