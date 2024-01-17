use std::time::Duration;

use cap_std::fs::Dir;

use tracing::debug;
use wasmtime::component::Linker;
use wasmtime::Engine;
use wasmtime_wasi::preview2::bindings::wasi;
use wasmtime_wasi::preview2::{
    DirPerms, FilePerms, StdinStream, StdoutStream, Table, WasiCtx, WasiCtxBuilder,
};

pub mod helpers;
pub mod logging;

pub fn create_linker<T>(engine: &Engine) -> wasmtime::Result<Linker<T>>
where
    T: Send
        + crate::preview2::wasi::blobstore::blobstore::Host
        + crate::preview2::wasi::blobstore::container::Host
        + crate::preview2::wasi::blobstore::types::Host
        + wasi::cli::environment::Host
        + wasi::cli::exit::Host
        + wasi::cli::stderr::Host
        + wasi::cli::stdin::Host
        + wasi::cli::stdout::Host
        + wasi::cli::terminal_input::Host
        + wasi::cli::terminal_output::Host
        + wasi::cli::terminal_stderr::Host
        + wasi::cli::terminal_stdin::Host
        + wasi::cli::terminal_stdout::Host
        + wasi::clocks::monotonic_clock::Host
        + wasi::clocks::wall_clock::Host
        + wasi::filesystem::preopens::Host
        + wasi::filesystem::types::Host
        + wasmtime_wasi_http::bindings::http::outgoing_handler::Host
        + wasmtime_wasi_http::bindings::http::types::Host
        + wasi::io::error::Host
        + wasi::io::streams::Host
        + wasi::io::poll::Host
        + crate::preview2::wasi::keyvalue::atomic::Host
        + crate::preview2::wasi::keyvalue::batch::Host
        + crate::preview2::wasi::keyvalue::cache::Host
        + crate::preview2::wasi::keyvalue::readwrite::Host
        + crate::preview2::wasi::keyvalue::types::Host
        + crate::preview2::wasi::keyvalue::wasi_cloud_error::Host
        + crate::preview2::wasi::logging::logging::Host
        + wasi::random::random::Host
        + wasi::random::insecure::Host
        + wasi::random::insecure_seed::Host
        + wasi::sockets::instance_network::Host
        + wasi::sockets::ip_name_lookup::Host
        + wasi::sockets::network::Host
        + wasi::sockets::tcp::Host
        + wasi::sockets::tcp_create_socket::Host
        + wasi::sockets::udp::Host
        + wasi::sockets::udp_create_socket::Host,
{
    let mut linker = Linker::new(engine);

    wasmtime_wasi::preview2::bindings::cli::environment::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::exit::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::stderr::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::stdin::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::stdout::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::terminal_input::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::terminal_output::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::terminal_stderr::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::terminal_stdin::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::terminal_stdout::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::clocks::monotonic_clock::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::clocks::wall_clock::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::filesystem::preopens::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::filesystem::types::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::io::error::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::io::poll::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::io::streams::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::random::random::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::random::insecure::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::random::insecure_seed::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::sockets::instance_network::add_to_linker(
        &mut linker,
        |x| x,
    )?;
    wasmtime_wasi::preview2::bindings::sockets::network::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::sockets::tcp::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::sockets::tcp_create_socket::add_to_linker(
        &mut linker,
        |x| x,
    )?;

    wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi_http::bindings::wasi::http::types::add_to_linker(&mut linker, |x| x)?;

    crate::preview2::wasi::blobstore::blobstore::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::blobstore::container::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::blobstore::types::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::atomic::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::batch::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::cache::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::readwrite::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::types::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::wasi_cloud_error::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::logging::logging::add_to_linker(&mut linker, |x| x)?;

    Ok(linker)
}

pub fn create_context<T, F>(
    args: &[impl AsRef<str>],
    env: &[(impl AsRef<str>, impl AsRef<str>)],
    root_dir: Dir,
    stdin: impl StdinStream + Sized + 'static,
    stdout: impl StdoutStream + Sized + 'static,
    stderr: impl StdoutStream + Sized + 'static,
    suspend_signal: impl Fn(Duration) -> anyhow::Error + Send + Sync + 'static,
    suspend_threshold: Duration,
    f: F,
) -> Result<T, anyhow::Error>
where
    F: FnOnce(WasiCtx, Table) -> T,
{
    let table = Table::new();
    debug!("Creating WASI context, root directory is {:?}", root_dir);
    let wasi = WasiCtxBuilder::new()
        .args(args)
        .envs(env)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .monotonic_clock(helpers::clocks::monotonic_clock())
        .preopened_dir(
            root_dir
                .try_clone()
                .expect("Failed to clone root directory handle"),
            DirPerms::all(),
            FilePerms::all(),
            "/",
        )
        .preopened_dir(root_dir, DirPerms::all(), FilePerms::all(), ".")
        .set_suspend(suspend_threshold, suspend_signal)
        .build();

    Ok(f(wasi, table))
}

#[cfg(test)]
mod tests;
