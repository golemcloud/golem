

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

get password

export GOLEM_POSTGRES_PASSWORD=$(kubectl get secret --namespace golem golem-postgres-postgresql -o jsonpath="{.data.password}" | base64 -d)



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


kubectl -n golem get service

service/golem-postgres-postgresql
service/golem-redis-master


```shell

helm template golem-chart --set postgres.password=$GOLEM_POSTGRES_PASSWORD --set env=golem

helm upgrade --install golem-default golem-chart -n golem --set postgres.password=$GOLEM_POSTGRES_PASSWORD --set env=golem
```

delete
```shell
helm delete -n golem golem-default
```