//! Wraps libc's path and descriptor entry points so `tools::devices` can answer `/dev` paths and
//! serve in-process commands' standard streams on WASI. Link
//! arguments reach only this package's own artifacts, so every package that links the shell into
//! a WASM binary carries the same script.
include!("../wrap_symbols.rs");

fn main() {
    println!("cargo:rerun-if-changed=../wrap_symbols.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        for symbol in WRAPPED_PATH_SYMBOLS {
            println!("cargo:rustc-link-arg=--wrap={symbol}");
        }
        // 4 MiB of shadow stack (Rust's default is 1 MiB), so recursion reaches ~90 levels before
        // Brush's nesting guard stops it. Much more would let Wasmtime's 512 KiB native stack run
        // out first, and that trap cannot be caught inside the component.
        println!("cargo:rustc-link-arg=-zstack-size=4194304");
    }
}
