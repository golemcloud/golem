// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Engine-boundary probes for blocking guest command execution.
//!
//! The opt-in fixture exercises QuickJS. Neither probe exercises durable tool
//! dispatch or the owner filesystem lane.
use anyhow::{Result, ensure};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use test_r::{test, timeout};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::error::Context as WasmtimeContext;
use wasmtime::{Engine, Store};
use wasmtime_wasi::{DirPerms, FilePerms, IoCtx, IoData, IoView, WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::WasiHttpCtx;
use wasmtime_wasi_http::p3::{WasiHttpCtxView, WasiHttpView};

test_r::enable!();

const CALLER: &str = r#"
(component
  (import "execute" (func $execute (param "value" u32) (result u32)))
  (core func $execute (canon lower (func $execute)))
  (core func $return (canon task.return (result u32)))
  (core module $caller
    (import "host" "execute" (func $execute (param i32) (result i32)))
    (import "host" "return" (func $return (param i32)))
    (func (export "run") (param $value i32) (result i32)
      local.get $value
      call $execute
      call $return
      i32.const 0)
    (func (export "callback") (param i32 i32 i32) (result i32) unreachable))
  (core instance $caller (instantiate $caller
    (with "host" (instance
      (export "execute" (func $execute))
      (export "return" (func $return))))))
  (func (export "run") async (param "value" u32) (result u32)
    (canon lift (core func $caller "run") async (callback (func $caller "callback")))))
"#;

const CHILD: &str = r#"
(component
  (core module $child
    (func (export "run") (param i32) (result i32)
      local.get 0
      i32.const 1
      i32.add))
  (core instance $child (instantiate $child))
  (func (export "run") (param "value" u32) (result u32)
    (canon lift (core func $child "run"))))
"#;

#[test]
#[timeout("30s")]
async fn blocking_import_from_async_export_can_await_a_nested_component() -> Result<()> {
    let engine =
        Engine::new(&golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache())?;
    let caller = Component::new(&engine, CALLER).context("compile async caller")?;
    let child = Arc::new(Component::new(&engine, CHILD).context("compile child")?);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut linker = Linker::<()>::new(&engine);
    let host_calls = calls.clone();
    let child_engine = engine.clone();
    linker
        .root()
        .func_wrap_async("execute", move |_store, (value,): (u32,)| {
            let calls = host_calls.clone();
            let engine = child_engine.clone();
            let child = child.clone();
            Box::new(async move {
                // Force suspension; an immediately-ready future proves less.
                tokio::task::yield_now().await;
                calls.fetch_add(1, Ordering::Relaxed);
                let mut child_store = Store::new(&engine, ());
                child_store.set_fuel(u64::MAX)?;
                child_store.set_epoch_deadline(u64::MAX);
                let child_linker = Linker::<()>::new(&engine);
                let instance = child_linker
                    .instantiate_async(&mut child_store, &child)
                    .await?;
                let run = instance.get_typed_func::<(u32,), (u32,)>(&mut child_store, "run")?;
                run.call_async(&mut child_store, (value,)).await
            })
        })?;
    let mut rows = Vec::new();
    for repetition in 0..5 {
        let mut store = Store::new(&engine, ());
        store.set_fuel(u64::MAX)?;
        store.set_epoch_deadline(u64::MAX);
        let instance = linker.instantiate_async(&mut store, &caller).await?;
        let run = instance.get_typed_func::<(u32,), (u32,)>(&mut store, "run")?;
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            store.run_concurrent(async |accessor| run.call_concurrent(accessor, (41,)).await),
        )
        .await
        .context("blocking host import timed out")?
        .context("async caller invoking blocking host import")??;
        ensure!(result == (42,), "unexpected child result: {result:?}");
        rows.push(serde_json::json!({
            "repetition": repetition,
            "elapsedMs": started.elapsed().as_secs_f64() * 1000.0,
            "result": result.0,
        }));
    }
    ensure!(calls.load(Ordering::Relaxed) == 5);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "rows": rows,
            "hostCalls": calls.load(Ordering::Relaxed),
            "caveat": "Production engine configuration; async-lifted caller, blocking guest import, yielding async host, fresh nested child Store. No QuickJS, filesystem, owner lane, streams, permissions or replay yet."
        }))?
    );
    Ok(())
}

struct ProbeHost {
    table: ResourceTable,
    wasi: WasiCtx,
    io: IoCtx,
    http: WasiHttpCtx,
}

struct ChildTask<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for ChildTask<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl IoView for ProbeHost {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn io_ctx(&mut self) -> &mut IoCtx {
        &mut self.io
    }

    fn io_data(&mut self) -> IoData<'_> {
        IoData {
            table: &mut self.table,
            io_ctx: &mut self.io,
        }
    }
}

impl WasiView for ProbeHost {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
            io_ctx: &mut self.io,
        }
    }
}

impl WasiHttpView for ProbeHost {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: Default::default(),
        }
    }
}

/// Build the fixture with wasm-rquickjs before running this test. Point
/// GOLEM_NODE_TOOL_PROBE_WASM at the resulting component. Tests never run compilers.
#[test]
#[ignore]
#[timeout("60s")]
async fn quickjs_blocking_import_can_await_a_nested_component() -> Result<()> {
    let engine =
        Engine::new(&golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache())?;
    let path = std::env::var("GOLEM_NODE_TOOL_PROBE_WASM")?;
    let caller = Component::from_file(&engine, path)?;
    let child = Arc::new(Component::new(&engine, CHILD)?);
    let host_calls = Arc::new(AtomicUsize::new(0));
    let child_calls = Arc::new(AtomicUsize::new(0));
    let mut linkers = Vec::new();
    for nested in [false, true] {
        let mut linker = Linker::<ProbeHost>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi::p3::add_to_linker(&mut linker)?;
        wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
        let child_engine = engine.clone();
        let child = child.clone();
        let host_calls = host_calls.clone();
        let child_calls = child_calls.clone();
        linker
            .instance("experiment:node-tools/host")?
            .func_wrap_concurrent("execute", move |_accessor, (value,): (u32,)| {
                let engine = child_engine.clone();
                let child = child.clone();
                host_calls.fetch_add(1, Ordering::Relaxed);
                let child_calls = child_calls.clone();
                Box::pin(async move {
                    tokio::task::yield_now().await;
                    if !nested {
                        return Ok((value + 1,));
                    }
                    // Store event loops cannot be driven recursively. Schedule the
                    // child independently, and cancel it if the caller disappears.
                    let mut task = ChildTask(tokio::spawn(async move {
                        child_calls.fetch_add(1, Ordering::Relaxed);
                        let mut store = Store::new(&engine, ());
                        store.set_fuel(u64::MAX)?;
                        store.set_epoch_deadline(u64::MAX);
                        let instance = Linker::<()>::new(&engine)
                            .instantiate_async(&mut store, &child)
                            .await?;
                        let run = instance.get_typed_func::<(u32,), (u32,)>(&mut store, "run")?;
                        run.call_async(&mut store, (value,)).await
                    }));
                    (&mut task.0).await.map_err(|error| {
                        wasmtime::Error::msg(format!("child task failed: {error}"))
                    })?
                })
            })?;
        linkers.push((nested, linker));
    }
    for repetition in 0..5 {
        for entry in ["run", "run-job"] {
            for (nested, linker) in &linkers {
                let fixture_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/node-tool-processes");
                let (wasi, io) = WasiCtx::builder()
                    .inherit_stdio()
                    .preopened_dir(fixture_root, "/", DirPerms::READ, FilePerms::READ)?
                    .build();
                let mut store = Store::new(
                    &engine,
                    ProbeHost {
                        table: ResourceTable::new(),
                        wasi,
                        io,
                        http: WasiHttpCtx::new(),
                    },
                );
                store.set_fuel(u64::MAX)?;
                store.set_epoch_deadline(u64::MAX);
                let instance = linker.instantiate_async(&mut store, &caller).await?;
                let run = instance.get_typed_func::<(u32,), (u32,)>(&mut store, entry)?;
                let started = Instant::now();
                let result = tokio::time::timeout(
                    Duration::from_secs(5),
                    store.run_concurrent(async |accessor| {
                        run.call_concurrent(accessor, (41,)).await
                    }),
                )
                .await
                .context("QuickJS blocking host import timed out")???;
                ensure!(result == (42,), "unexpected child result: {result:?}");
                println!(
                    "{}",
                    serde_json::json!({
                        "repetition": repetition,
                        "entry": entry,
                        "nested": nested,
                        "elapsedMs": started.elapsed().as_secs_f64() * 1000.0,
                        "result": result.0,
                        "caveat": "QuickJS, synchronous guest import, concurrent host, fresh nested child Store. Not real tool dispatch or owner filesystem."
                    })
                );
            }
        }
    }
    ensure!(host_calls.load(Ordering::Relaxed) == 20);
    ensure!(child_calls.load(Ordering::Relaxed) == 10);
    Ok(())
}
