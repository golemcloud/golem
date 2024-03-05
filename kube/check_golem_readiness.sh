#!/bin/bash

usage() { echo "Usage: $0 -n <NAMESPACE>" 1>&2; exit 1; }

while getopts "n:" o; do
    case "${o}" in
        n)
            NAMESPACE=${OPTARG}
            ;;
        *)
            usage
            ;;
    esac
done

if [[ -z "$NAMESPACE" ]]; then
    usage
fi

kubectl get namespace $NAMESPACE > /dev/null 2>&1
if [ $? -ne 0 ]; then
  echo "Namespace '$NAMESPACE' does not exist"
  exit 1
fi

echo "Checking golem readiness in namespace: $NAMESPACE"

required_pod_substrings=("shard-manager" "worker-executor" "worker-service" "template-service")
counter=0
timeout=4

while true; do
  missing=false
  for substring in "${required_pod_substrings[@]}"; do
    if ! kubectl get pods --namespace $NAMESPACE -o go-template='{{range $index, $element := .items}}{{range .status.containerStatuses}}{{if .ready}}{{$element.metadata.name}}{{"\n"}}{{end}}{{end}}{{end}}' | grep -q "$substring"; then
      echo "Required pod with substring '$substring' is missing."
      missing=true
      break
    fi
  done

  if [ "$missing" == "false" ]; then
    echo "Required pods are ready"
    break
  fi

  counter=$((counter + 1))
  if [ $counter -ge $timeout ]; then
    echo "Timeout: Pods did not become ready."
    exit 1
  fi

  sleep 10
  echo "Waiting for pods to become ready ..."
done
