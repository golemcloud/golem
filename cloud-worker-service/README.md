## Worker Service


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