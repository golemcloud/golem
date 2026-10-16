//! Standalone `waget` CLI, a development tool: the embeddable library ([`waget::run`]) does the
//! work, and this wrapper forwards argv and writes the outcome. On wasm it is a WASI 0.3 command
//! whose async `wasi:cli/run@0.3.0` export (which `wasmtime run -Sp3` calls) awaits the request:
//! WASI-HTTP's futures can be awaited only in a component-model async task, and a synchronous
//! `main` may not block on them. Natively a small tokio runtime runs it.

use std::io::Write;

/// Writes the outcome and exits with its status.
fn finish(outcome: waget::Outcome) -> ! {
    let _ = std::io::stdout().write_all(&outcome.stdout);
    let _ = std::io::stderr().write_all(&outcome.stderr);
    std::process::exit(i32::from(outcome.exit_code));
}

#[cfg(target_arch = "wasm32")]
struct Command;

#[cfg(target_arch = "wasm32")]
impl wasip3::exports::wasi::cli::run::Guest for Command {
    async fn run() -> Result<(), ()> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        finish(waget::run(&args).await)
    }
}

#[cfg(target_arch = "wasm32")]
wasip3::cli::command::export!(Command);

/// Unused on wasm, where `wasmtime run -Sp3` calls the async export above instead.
#[cfg(target_arch = "wasm32")]
fn main() {
    eprintln!("waget: run it with `wasmtime run -Sp3 -Shttp`, which calls its async export");
    std::process::exit(1);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Runtime construction failure is unrecoverable in the standalone CLI: fail fast.
    #[allow(clippy::expect_used)]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    finish(runtime.block_on(waget::run(&args)))
}
