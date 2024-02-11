create namespace

```shell
kubectl create namespace golem
```

## postgres

https://artifacthub.io/packages/helm/bitnami/postgresql

install
```shell
helm install -n golem golem-postgres oci://registry-1.docker.io/bitnamicharts/postgresql --set auth.database=golem_db --set auth.username=golem_user
```

delete
```shell
helm delete -n golem golem-postgres
```

get password (if you need it)
```shell
export GOLEM_POSTGRES_PASSWORD=$(kubectl get secret --namespace golem golem-postgres-postgresql -o jsonpath="{.data.password}" | base64 -d)
```

## redis

https://artifacthub.io/packages/helm/bitnami/redis

install
```shell
helm install -n golem golem-redis oci://registry-1.docker.io/bitnamicharts/redis --set auth.enabled=false
```

delete
```shell
helm delete -n golem golem-redis
```

```shell
kubectl -n golem get service

service/golem-postgres-postgresql
service/golem-redis-master
```

## golem services

install
```shell
helm upgrade --install golem-default golem-chart -n golem
```

show kube files
```shell

helm template golem-chart
```

delete
```shell
helm delete -n golem golem-default
```