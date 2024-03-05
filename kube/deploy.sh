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

INGRESS_NAMESPACE=ingress-nginx

kubectl get namespace $INGRESS_NAMESPACE > /dev/null 2>&1
if [ $? -ne 0 ]; then
  echo "Installing ingress-nginx to namespace $INGRESS_NAMESPACE"
  helm upgrade --install ingress-nginx ingress-nginx --repo https://kubernetes.github.io/ingress-nginx --namespace $INGRESS_NAMESPACE --create-namespace
else
  echo "ingress-nginx namespace $INGRESS_NAMESPACE already exists, do not executing installation"
fi

echo ""
echo "Creating namespace $NAMESPACE"

kubectl create namespace $NAMESPACE

echo ""
echo "Installing postgres to namespace $NAMESPACE"

helm upgrade --install -n $NAMESPACE golem-postgres oci://registry-1.docker.io/bitnamicharts/postgresql --set auth.database=golem_db --set auth.username=golem_user

echo ""
echo "Installing redis to namespace $NAMESPACE"

helm upgrade --install -n $NAMESPACE golem-redis oci://registry-1.docker.io/bitnamicharts/redis --set auth.enabled=false

echo ""
echo "Waiting 30s for services to startup up ..."

sleep 30

echo ""
echo "Installing golem to namespace $NAMESPACE"

helm upgrade --install golem-default golem-chart -n $NAMESPACE

echo ""
echo "Waiting 30s for golem to startup up ..."

sleep 30

echo ""
./check_golem_readiness.sh -n $NAMESPACE
if [[ $? -ne 0 ]]; then
  echo "Checking golem readiness namespace: $NAMESPACE failed"
fi

echo ""
echo "Installation done"

echo ""
echo "To show all kubernetes components for namespace $NAMESPACE, run:"
echo "kubectl -n $NAMESPACE get all"

echo ""
echo "To setup GOLEM_BASE_URL for golem-cli, run:"
echo "export GOLEM_BASE_URL=http://localhost:80"

echo ""
