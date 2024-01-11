# golem-services

This repository contains the open source parts of Golem Cloud - a set of services enable you to run WebAssembly components in a distributed cloud environment.

See [Golem Cloud](https://golem.cloud) for more information.

## Local Testing

To spin up services

```bash
docker-compose up
```

Note: docker-compose up alone will not rebuild the changes.
To compose up with changes:

```bash
docker-compose up --build
```
