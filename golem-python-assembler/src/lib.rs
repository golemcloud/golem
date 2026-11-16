//! Builds Golem components from prepared Python assets.
//!
//! The prepared assets carry the CPython runtime and the bindings for the
//! Python SDK's WIT world; see `componentize_py::prepared`. An application
//! build links them with the SDK sources and the application sources, then
//! pre-initializes the result on Golem's shared Wasmtime engine. The build
//! does not process WIT, generate code or compile CPython.

use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use componentize_py::prepared::{Assembler, Prepared};
use wasmtime::{
    Engine, Store,
    component::{Linker, ResourceTable},
};
use wasmtime_wasi::{
    Deterministic, FsPerms, IoCtx, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView,
    p2::pipe::MemoryOutputPipe,
};

#[cfg(test)]
test_r::enable!();

/// The SDK module that pre-initialization imports.
pub const BOOTSTRAP_MODULE: &str = "golem._bootstrap";

/// The environment variable that passes the entry modules to the bootstrap.
pub const ENTRY_MODULES_VAR: &str = "GOLEM_PYTHON_ENTRY_MODULES";

/// The size of the linear-memory C stack.
///
/// CPython raises `RecursionError` when this stack is nearly full, but it does
/// not see the engine's call stack, which the shared engine limits to 1 MiB.
/// With 96 KiB, deep `repr`, `json`, `pickle` and `__repr__` recursion raises
/// `RecursionError` instead of exhausting the engine's call stack.
pub const STACK_SIZE: u32 = 96 * 1024;

/// The capacity of each of the stdout and stderr buffers of
/// pre-initialization. Writing more fails the build.
pub const OUTPUT_CAPACITY: usize = 64 * 1024;

/// The guest mount names of the SDK and the application sources.
const SDK_MOUNT: &str = "sdk";
const APP_MOUNT: &str = "app";

/// An application to build.
pub struct Application<'a> {
    /// The directory that contains the SDK's `golem` package.
    pub sdk_dir: &'a Path,
    /// The directory that contains the application's modules.
    pub source_dir: &'a Path,
    /// The modules that declare the application's agents, imported in order.
    pub entry_modules: &'a [&'a str],
}

struct Host {
    ctx: WasiCtx,
    io: IoCtx,
    table: ResourceTable,
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
            io_ctx: &mut self.io,
        }
    }
}

/// The clock seen during pre-initialization. It starts at the Unix epoch and
/// advances one microsecond each time it is read, so nothing the application
/// saves depends on when it was built.
#[derive(Default)]
struct BuildClock(AtomicU64);

impl BuildClock {
    fn tick(&self) -> u64 {
        self.0.fetch_add(1000, Ordering::Relaxed)
    }
}

impl wasmtime_wasi::HostWallClock for BuildClock {
    fn resolution(&self) -> Duration {
        Duration::from_micros(1)
    }

    fn now(&self) -> Duration {
        Duration::from_nanos(self.tick())
    }
}

impl wasmtime_wasi::HostMonotonicClock for BuildClock {
    fn resolution(&self) -> u64 {
        1000
    }

    fn now(&self) -> u64 {
        self.tick()
    }
}

fn validate_entry_module(module: &str) -> Result<()> {
    ensure!(
        !module.is_empty()
            && module.split('.').all(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                    && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
            }),
        "`{module}` is not a Python module name"
    );
    Ok(())
}

/// Reads and verifies prepared assets.
pub fn read_prepared(dir: &Path) -> Result<Prepared> {
    Prepared::read(dir)
}

/// Builds the component for `app` and returns its bytes.
///
/// Fails before any application code runs when the entry modules are invalid.
/// An exception that an entry module raises during import fails the build,
/// and the error contains its traceback and the output of pre-initialization.
pub async fn build(prepared: &Prepared, app: &Application<'_>) -> Result<Vec<u8>> {
    ensure!(
        !app.entry_modules.is_empty(),
        "the application has no entry modules"
    );
    for module in app.entry_modules {
        validate_entry_module(module)?;
    }

    let python_path = [(SDK_MOUNT, app.sdk_dir), (APP_MOUNT, app.source_dir)];
    let assembly = Assembler {
        prepared,
        app_name: BOOTSTRAP_MODULE,
        python_path: &python_path,
        stack_size: Some(STACK_SIZE),
    }
    .assemble()?;

    let plan = &assembly.plan;
    let stdout = MemoryOutputPipe::new(OUTPUT_CAPACITY);
    let stderr = MemoryOutputPipe::new(OUTPUT_CAPACITY);
    let mut wasi = WasiCtxBuilder::new();
    wasi.stdout(stdout.clone()).stderr(stderr.clone());
    wasi.secure_random(Deterministic::new(plan.entropy.random_bytes.clone()))
        .insecure_random(Deterministic::new(plan.entropy.random_bytes.clone()))
        .insecure_random_seed(plan.entropy.insecure_seed)
        .wall_clock(BuildClock::default())
        .monotonic_clock(BuildClock::default())
        .env(ENTRY_MODULES_VAR, app.entry_modules.join(","));
    for (key, value) in &plan.env {
        wasi.env(key, value);
    }
    for preopen in &plan.preopens {
        wasi.preopened_dir(&preopen.host, &preopen.guest, FsPerms::ReadWrite)
            .map_err(|error| {
                anyhow::Error::from(error).context(preopen.host.display().to_string())
            })?;
    }
    let (ctx, io) = wasi.build();

    let engine =
        Engine::new(&golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache())?;
    let mut linker = Linker::<Host>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    componentize_py::add_trapping_stubs(&mut linker, &plan.stubs)?;

    let mut store = Store::new(
        &engine,
        Host {
            ctx,
            io,
            table: ResourceTable::new(),
        },
    );
    // The shared engine meters fuel and interrupts on epochs; initialization
    // runs to completion.
    store.set_fuel(u64::MAX)?;
    store.set_epoch_deadline(u64::MAX);

    // The runtime prints the traceback of an exception that fails
    // initialization to stderr; the error itself names only the exception.
    assembly.initialize(store, linker).await.with_context(|| {
        format!(
            "Python pre-initialization failed\n{}{}",
            String::from_utf8_lossy(&stdout.contents()),
            String::from_utf8_lossy(&stderr.contents())
        )
    })
}

#[cfg(test)]
mod tests {
    use super::validate_entry_module;
    use test_r::test;

    #[test]
    fn entry_module_names() {
        for valid in ["app", "my_app.agents", "_private", "a1.b2"] {
            assert!(validate_entry_module(valid).is_ok(), "{valid}");
        }
        for invalid in ["", "1app", "app.", ".app", "a,b", "a-b", "a..b", "ä"] {
            assert!(validate_entry_module(invalid).is_err(), "{invalid}");
        }
    }
}
