# golem-wasm-rpc-stubgen

The `golem-wasm-rpc-stubgen` is a CLI tool to generate the RPC stubs from a component's WIT definition.

```shell
Usage: wasm-rpc-stubgen generate [OPTIONS] --source-wit-root <SOURCE_WIT_ROOT> --dest-crate-root <DEST_CRATE_ROOT>

Options:
  -s, --source-wit-root <SOURCE_WIT_ROOT>                
  -d, --dest-crate-root <DEST_CRATE_ROOT>                
  -w, --world <WORLD>                                    
      --stub-crate-version <STUB_CRATE_VERSION>          [default: 0.0.1]
      --wasm-rpc-path-override <WASM_RPC_PATH_OVERRIDE>  
  -h, --help                                             Print help
  -V, --version                                          Print version
```

- `source-wit-root`: The root directory of the component's WIT definition to be called via RPC
- `dest-crate-root`: The target path to generate a new stub crate to
- `world`: The world name to be used in the generated stub crate. If there is only a single world in the source root
  package, no need to specify.
- `stub-crate-version`: The crate version of the generated stub crate
- `wasm-rpc-path-override`: The path to the `wasm-rpc` crate to be used in the generated stub crate. If not specified,
  the latest version of `wasm-rpc` will be used.

The command creates a new Rust crate that is ready to be compiled with

```shell
cargo component build --release
```

The resulting WASM component implements the **stub interface** corresponding to the source interface, found in the
target directory's
`wit/_stub.wit` file. This WASM component is to be composed together with another component that calls the original
interface via WASM RPC.