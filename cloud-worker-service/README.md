## Worker Service

### Run Worker Service in Local

Note that, we will be removing the need of all these configurations for local set up soon.

```bash
export GOLEM__REDIS__HOST="localhost"
export GOLEM__REDIS__PORT="1234"
export GOLEM__REDIS__DATABASE="1"
export GOLEM__ENVIRONMENT="local"
export GOLEM__WORKSPACE="release"
export GOLEM__COMPONENT_SERVICE__HOST="localhost"
export GOLEM__COMPONENT_SERVICE__PORT="1234"
export GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN="token"
export GOLEM__ROUTING_TABLE__HOST="localhost"
export GOLEM__ROUTING_TABLE__PORT="1234"
cargo run

```


### Certificates

generate certificate for domain

```bash
sh generate-cert.sh golem.cloud.test
```


cert check for cart subdomain
```bash
openssl s_client -connect k8s-devpr102-ingressa-fa747fd214-789428298.us-east-1.elb.amazonaws.com:443 -servername cart.golem.cloud.test
```

example usage for cart subdomain
```
curl --cacert golem.cloud.test.pem  --connect-to cart.golem.cloud.test:443:k8s-devpr102-ingressa-fa747fd214-789428298.us-east-1.elb.amazonaws.com:443 https://cart.golem.cloud.test/items/1 --tls-max 1.2 -v
```