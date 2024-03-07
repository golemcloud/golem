# golem-cli

Command line interface for [Golem OSS](https://golem.cloud).

For Golem Cloud version client see [Golem Cloud CLI](https://github.com/golemcloud/golem-cloud-cli).

## Installation

To install `golem-cli` you currently need to use `cargo`, Rust's build tool.

To get `cargo` on your system, we recommend to use [rustup](https://rustup.rs/):

```shell
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup install stable
rustup default stable
```

Another external dependency is `protobuf`, with a minimum version of `3.15`,
which can be installed as described on http://google.github.io/proto-lens/installing-protoc.html. On macOS, you can install it with Homebrew:

```shell
brew install protobuf
```

Then you can install `golem-cli` with the following command:

```shell
cargo install golem-cli
```

## More information

Please check the [Golem Cloud developer documentation portal](https://learn.golem.cloud) to learn more about how to get started with _Golem Cloud_!

## Contributing

Find details [here](CONTRIBUTING.md)
