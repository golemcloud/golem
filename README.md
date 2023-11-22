# golem-wasm-ast
<p>
    <a href="https://crates.io/crates/golem-wasm-ast">
        <img src="https://img.shields.io/crates/v/golem-wasm-ast.svg" alt="Crate"/>
    </a>
    <a href="https://docs.rs/golem-wasm-ast/latest/">
        <img src="https://docs.rs/golem-wasm-ast/badge.svg" alt="Docs"/>
    </a>
</p>

Higher level WASM library for Rust

This library defines an in-memory, mutable representation of WebAssembly modules and [components](https://github.com/WebAssembly/component-model). It uses  
the [wasmparser](https://crates.io/crates/wasmparser) and [wasm-encoder](https://crates.io/crates/wasm-encoder) crates for building up and serializing this model.

Building up the full AST in memory makes it easier to perform analysis and mutation on a whole WASM component. 
The `analysis` module defines such higher level operations.

## Usage
Add wasm-ast to your Cargo.toml

```shell
$ cargo add golem-wasm-ast
```

Then parse a WASM module or component from an array of bytes:

```rust
use std::fmt::Debug;
use golem_wasm_ast::DefaultAst;
use golem_wasm_ast::analysis::AnalysisContext;
use golem_wasm_ast::core::{Expr, Module};
use golem_wasm_ast::component::Component;

fn main() {
    let module_bytes: Vec<u8> = ...;
    let module: Module<DefaultAst> = Component::from_bytes(&component_bytes).unwrap();
    
    let component_bytes: Vec<u8> = ...;
    let component: Component<DefaultAst> = Component::from_bytes(&component_bytes).unwrap();

    println!("component metadata {:?}", component.get_metadata());

    let state = AnalysisContext::new(component);
    let analysed_exports = state.get_top_level_exports().unwrap();
    println!("analysed exports: {:?}", analysed_exports);

}
```

Use the top level `Module` or `Component` structs to query and manipulate parts of the model. 
It is possible to use a different type than `Expr`, `Data` and `Custom` to represent the code blocks, data and custom sections in the parsed AST to reduce the memory footprint in case the actual code is not required for the analysis. Note that if this custom representation cannot be serialized back to a stream of WASM instructions, the AST will no longer be serializable.

The following example just ignores all the code blocks, but keeps the data and custom sections:

```rust
#[derive(Debug, Clone, PartialEq)]
struct IgnoredExpr {}

impl TryFromExprSource for IgnoredExpr {
    fn try_from<S: ExprSource>(_value: S) -> Result<Self, String>
    where
        Self: Sized,
    {
        Ok(IgnoredExpr {})
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CustomAst;

impl AstCustomization for CustomAst {
    type Expr = IgnoredExpr;
    type Data = Data<IgnoredExpr>;
    type Custom = Custom;
}

fn main() {
    let module_bytes: Vec<u8> = ...;
    let module: Module<CustomAst> = Component::from_bytes(&component_bytes).unwrap();
}
```

It is possible to do some parse-time analysis of the code blocks in the `TryFromExprSource` implementation and store the analysation result in place of the `Expr` nodes.

## Features
- `component` enables support for the WASM Component Model 
- `parser` enables parsing of WASM modules and components
- `writer` enables the serialization of WASM modules and components
- `metadata` enables the parsing of WASM metadata sections using the [wasm-metadata](https://crates.io/crates/wasm-metadata) crate
- `analysis` enables higher level analysis and mutation of WASM modules and components

The `default` feature enables all the above.
