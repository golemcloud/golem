## Running integration tests

Integration tests are stored in `tests` directory.

You can run all tests with
```shell
./scripts/it.sh
```

To run individual tests you should first build all executables with `./scripts/build-all.sh` and then run tests by name:
```shell
GOLEM_DOCKER_SERVICES=true GOLEM_TEST_TEMPLATES="./test-templates" cargo test --test integration worker_new_instance
```

With `QUIET=true` you can hide services output:
```shell
QUIET=true GOLEM_DOCKER_SERVICES=true GOLEM_TEST_TEMPLATES="./test-templates"  cargo test --test integration
```

This way tests will use configured versions of golem docker images.
To run tests against the latest binaries without docker - see [`golem-services` CONTRIBUTING.md](https://github.com/golemcloud/golem-services/blob/main/CONTRIBUTING.md)

## Running sharding tests

Same as integration tests, but with `--test sharding` instead of `--test integration`:
```shell
QUIET=true GOLEM_DOCKER_SERVICES=true GOLEM_TEST_TEMPLATES="./test-templates"  cargo test --test sharding
```
