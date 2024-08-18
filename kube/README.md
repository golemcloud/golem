## Deployment

You will need to have [Helm](https://helm.sh/docs/intro/quickstart/) and [kubectl](https://kubernetes.io/docs/tasks/tools/) installed locally
and running [kubernetes](https://kubernetes.io/docs/concepts/overview/) cluster.

provided script:

```shell
./deploy.sh -n golem
```

will deploy Golem with Redis, PostgreSQL and Nginx ingress to kubernetes namespace `golem`. Kubernetes [Persistent Volumes](https://kubernetes.io/docs/concepts/storage/persistent-volumes/) are used to store Golem's data.

notes:
* by default, ingress is exposed under localhost:80, if you want to test golem-services in kubernetes locally (docker with kubernetes),
  and try to run commands with `golem-cli`, you may need to change `golem-cli` configuration to use that URL.
* by default, worker API Gateway endpoint is exposed under port 9006

## Deployment in steps

create namespace

```shell
kubectl create namespace golem
```

### PostgreSQL

https://artifacthub.io/packages/helm/bitnami/postgresql

install
```shell
helm upgrade --install -n golem golem-postgres oci://registry-1.docker.io/bitnamicharts/postgresql --set auth.database=golem_db --set auth.username=golem_user
```

delete
```shell
helm delete -n golem golem-postgres
```

get password (if you need it)
```shell
export GOLEM_POSTGRES_PASSWORD=$(kubectl get secret --namespace golem golem-postgres-postgresql -o jsonpath="{.data.password}" | base64 -d)
```

### Redis

https://artifacthub.io/packages/helm/bitnami/redis

install
```shell
helm upgrade --install -n golem golem-redis oci://registry-1.docker.io/bitnamicharts/redis --set auth.enabled=true
```

delete
```shell
helm delete -n golem golem-redis
```

get password (if you need it)

```shell
export REDIS_PASSWORD=$(kubectl get secret --namespace golem golem-redis -o jsonpath="{.data.redis-password}" | base64 -d)
```

```shell
kubectl -n golem get service

service/golem-postgres-postgresql
service/golem-redis-master
```

### ngnix ingress

install
```shell
helm upgrade --install ingress-nginx ingress-nginx --repo https://kubernetes.github.io/ingress-nginx --namespace ingress-nginx --create-namespace
```

you can watch the status by running

```shell
kubectl get service --namespace ingress-nginx ingress-nginx-controller --output wide --watch
```

### Golem services

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

shell to the running pod/container
```shell
kubectl exec --stdin --tty -n golem  <pod> -- /bin/bash
```

scale worker executor deployment
```shell
kubectl -n golem scale deployment.apps/deployment-worker-executor-default --replicas=3
```
