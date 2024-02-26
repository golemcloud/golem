#!/bin/bash

NAMESPACE=golem

INGRESS_NAMESPACE=ingress-nginx

echo "Installing ingress-nginx to namespace $INGRESS_NAMESPACE"

helm upgrade --install ingress-nginx ingress-nginx --repo https://kubernetes.github.io/ingress-nginx --namespace $INGRESS_NAMESPACE --create-namespace

echo ""
echo "Creating namespace $NAMESPACE"

kubectl create namespace $NAMESPACE

echo ""
echo "Installing postgres to namespace $NAMESPACE"

helm upgrade --install -n $NAMESPACE golem-postgres oci://registry-1.docker.io/bitnamicharts/postgresql --set auth.database=golem_db --set auth.username=golem_user

echo ""
echo "Installing postgres to namespace $NAMESPACE"

helm upgrade --install -n $NAMESPACE golem-redis oci://registry-1.docker.io/bitnamicharts/redis --set auth.enabled=false

echo ""
echo "Installing golem to namespace $NAMESPACE"

helm upgrade --install golem-default golem-chart -n $NAMESPACE

echo ""
echo "Installation done"

echo ""
echo "To show all kubernetes components for namespace $NAMESPACE, run:"
echo "kubectl -n $NAMESPACE get all"

echo ""
echo "To setup GOLEM_BASE_URL for golem-cli, run:"
echo "export GOLEM_BASE_URL=http://localhost:80"

echo ""
