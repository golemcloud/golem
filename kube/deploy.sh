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
echo "Installing PostgreSQL to namespace $NAMESPACE"

helm upgrade --install -n $NAMESPACE golem-postgres oci://registry-1.docker.io/bitnamicharts/postgresql --set auth.database=golem_db --set auth.username=golem_user

echo ""
echo "Installing Redis to namespace $NAMESPACE"

helm upgrade --install -n $NAMESPACE golem-redis oci://registry-1.docker.io/bitnamicharts/redis --set auth.enabled=true

echo ""
echo "Waiting 30s for services to startup up ..."

sleep 30

echo ""
echo "Installing Golem to namespace $NAMESPACE"

kubectl create serviceaccount -n $NAMESPACE golem-sa-default

helm upgrade --install golem-default golem-chart -n $NAMESPACE

echo ""
echo "Waiting 30s for Golem to startup up ..."

sleep 30

echo ""
./check_golem_readiness.sh -n $NAMESPACE
if [[ $? -ne 0 ]]; then
  echo "Checking Golem readiness namespace: $NAMESPACE failed"
fi

echo ""
echo "Installation done"

echo ""
echo "To show all kubernetes components for namespace $NAMESPACE, run:"
echo "kubectl -n $NAMESPACE get all"

echo ""
echo "Use http://localhost:80 in golem-cli"

echo ""
