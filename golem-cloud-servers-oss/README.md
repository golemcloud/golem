## Docker build
Make sure you run docker build from the root directory of golem-services.

```bash

docker build --build-arg SHARD_MANAGER_HOST=localhost --build-arg SHARD_MANAGER_PORT=9000 --build-arg TEMPLATES__STORE__ROOT_PATH=myfile -t somethingss -f golem-cloud-servers-oss/Dockerfile .

```