# golem-services

This repository contains the open source parts of Golem Cloud - a set of services enable you to run WebAssembly components in a distributed cloud environment.

See [Golem Cloud](https://golem.cloud) for more information.

## Local Testing

To spin up services

```bash
docker-compose up
```

Note that docker-compose up alone will not rebuild the changes.

We avoid Rust base images and cargo usage within docker for faster builds.
Therefore, for changes to reflect:

```bash
# from the root of the project
cargo build
docker-compose up 
```