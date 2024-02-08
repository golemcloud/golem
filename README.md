# wasm-rpc

The crate can be both used in host and guest environments:

To compile the host version:
```shell
cargo build --no-default-features --features host
```

To compile the guest version, has minimal dependencies and feature set to be used in generated stubs:
```shell
cargo component build --no-default-features --features stub
```
