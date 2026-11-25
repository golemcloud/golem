# Build the example images

These examples use the settings of this checkout. The published 1.6 images do not read them. Build the Golem images from the same checkout before you start an example. The examples select the local tag `local` and do not pull Golem images. Docker still pulls their base images and the images of dependencies such as PostgreSQL and Caddy.

Use a Linux host with the [build prerequisites](../AGENTS.md#prerequisites). The architecture of the binaries must match the Docker host. From the repository root:

```sh
unset GOLEM_BUILTIN_RELEASE
CARGO_TARGET_DIR=target cargo make ensure-builtin-artifacts
case "$(uname -m)" in
  x86_64) target=x86_64-unknown-linux-gnu ;;
  aarch64|arm64) target=aarch64-unknown-linux-gnu ;;
  *) echo "Use an x86-64 or ARM64 Linux host" >&2; exit 1 ;;
esac
CARGO_TARGET_DIR=target cargo build --release --target "$target" --bins \
  --features golem/embed-builtin-artifacts,golem-registry-service/embed-builtin-artifacts \
  -p golem -p golem-registry-service -p golem-shard-manager \
  -p golem-worker-service -p golem-component-compilation-service -p golem-worker-executor
docker compose --env-file docker-examples/published-postgres/.env \
  -f docker-examples/published-postgres/compose.yaml build
docker build -f cli/golem/docker/Dockerfile -t golemservices/golem:local .
```

The first task prepares the built-in artifacts. The build embeds them in the registry and all-in-one binaries, so fresh containers do not need paths from the build host. This local build does not use the release-download mode. The Compose build packages the five service binaries and the router. The last command packages the all-in-one binary. The Dockerfiles copy the prebuilt Linux binaries from `target/<target>/release`; they do not compile Rust.

Then follow the selected example:

- [Single node](single-node/README.md), with the all-in-one image, Caddy and an external S3 bucket.
- [Fixed cluster](production-cluster/README.md), with external PostgreSQL and S3.
- `published-postgres/compose.yaml`, a local development cluster with example credentials and PostgreSQL.
- [Distributed shard manager](distributed-etcd/README.md), a local development cluster with etcd.

After you change the checkout, rebuild the binaries and images together. Do not point these settings at an older published image. The local build commands do not publish an image or change a running cluster.
