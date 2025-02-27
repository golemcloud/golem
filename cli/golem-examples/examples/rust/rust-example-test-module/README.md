# Synopsis

This project serves as an example/template for building an application using [WebAssembly Component Model](https://github.com/webassembly/component-model) (and in this case, an app for Golem Cloud).  It is my recommendation on how to structure such a project.

## Problem statement

The [`cargo-component`](https://github.com/bytecodealliance/cargo-component) project by Bytecode Alliance greatly smooths out the development process in building WebAssembly Component Model applications (in Rust).  You will define your app's public data structures (records and enums) and function interfaces in a [`wit` (Wasm interface type)](https://component-model.bytecodealliance.org/design/wit.html) file, which is then used to generate the Rust bindings for your Wasm component.  The generated code sits in a module somewhere inside the `target` directory.  Running `cargo component build` will build a valid codebase successfully.

Sadly, as of this writing, the regular Rust tooling (e.g. rust-analyzer) lacks visibility to this module.  As a result, red squiggly lines will likely appear in our IDE for any data types defined in the `wit` file. Additionally, `cargo test` will fail because it cannot resolve those references.

This is the motivation of creating a project structure that allows us to follow the development process we have accustomed to: write and run tests locally, as well as running them on CI.

Although discussing the merits (and the associated costs) of writing (and maintaining) tests is out of scope here, the ability to test our apps locally in this case can be quite desireable.  Even though it's feasible to test and debug our apps directly in the cloud, the feedback loop of such practice is much slower, let alone the time wasted on rebuilding and redeploying apps.  This project (template) makes it possible to run tests without having to change or disable regions of our Wasm component code merely for testing purposes.  It's my current recommendation on building WASI apps until `cargo-component` improves (or better alternatives become available).

Before I start the walkthrough and explain the project structure, make sure you have set up Rust's toolchain and installed `cargo-component`. Please refer to [Golem Cloud documentation](https://www.golem.cloud/learn/rust) for instructions.

## Workspace structure

### 1. Core module

The core business logic of our application goes into the `lib` module, where our code can be organized into logical units and only a select set of functions (and structs) are exposed as public APIs. This is quite common in Rust applications.

Like most Rust apps, we can write unit tests in each sub-module as well as integration tests in a separate `tests` module.

To run tests at the project's root, simply do `cargo nextest run -p lib`. (I highly recommend using `cargo-nextest` as the test runner.)  Note the `-p` parameter at the end of the command -- we are passing the name of this module as the value.

We can also check test coverage by running `cargo tarpaulin -p lib`. (Run `cargo install cargo-tarpaulin` to add the sub-command.)

### 2. Console app

This one is optional.  However, I feel it's beneficial to build a console app, which not only provides us a way to system-test our APIs defined in the `lib` module but, rather importantly, it can guide us through the process of designing the APIs, especially in the early iterations.  While calling the APIs in a console app might be slightly different than calling them from the Wasm component (because the execution flow may likely be different), we can still try to mimic, as much as possible, the cloud API flow (exposed by the Wasm component -- we're speaking in the context of running it on Golem Cloud here).  By doing so, this console app shall give us a very close feel of how our APIs will work.  This will provide a fast feedback loop and allow us to iterate quicker and with more confidence; at the same time it should help minimize the potential of a bad API design.

To run the console app at the project root, do `cargo run -p app`.  Again, we will pass the `-p` parameter and specify the `app` module.  If you want to produce a binary of the console app, running `cargo build -p app` will produce the executable as `target/debug/app` (or `target/release/app` if the `--release` flag is included in the build command).

### 3. Wasm app

Now that we have tests and the console app to guide the implementation, we should have a very good idea of how our APIs will look like.  Thereby, we shall express our APIs in the `wit` file.  Next, we will add some boilerplate code in the `wasm` module to glue the Rust bindings to our Wasm implementation. Please refer to the code in `wasm\lib.rs` as well as Golem's documentation.  The implementation in the Wasm module should be fairly trivial and quite similar to that of the console app.

To build the Wasm assembly, run `cargo component build --release -p wasm` at the root of our project directory.  This will produce the `target/wasm32-wasip1/release/lib.wasm` file in this case.
