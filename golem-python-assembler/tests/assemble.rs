//! Builds Python applications from the prepared SDK assets and runs them on
//! the shared engine.
//!
//! The prepared assets are a build output; `cargo make python-sdk-prepare`
//! writes them to `sdks/python/target/prepared`.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use golem_python_assembler::{Application, build, read_prepared};
use test_r::test;
use wasmtime::{
    Engine, Store,
    component::{Component, Linker, ResourceTable, Val},
};
use wasmtime_wasi::{IoCtx, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

test_r::enable!();

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

fn python_sdk() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sdks/python")
}

fn prepared() -> Result<componentize_py::prepared::Prepared> {
    let dir = python_sdk().join("target/prepared");
    read_prepared(&dir).with_context(|| {
        format!(
            "run `cargo make python-sdk-prepare` to write the prepared assets to {}",
            dir.display()
        )
    })
}

fn write_app(dir: &std::path::Path, module: &str, source: &str) -> Result<()> {
    fs::write(dir.join(format!("{module}.py")), source)?;
    Ok(())
}

async fn discover_agent_types(component: &[u8]) -> Result<Val> {
    let engine =
        Engine::new(&golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache())?;
    let component = Component::new(&engine, component)?;
    let mut linker = Linker::<Host>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    linker.define_unknown_imports_as_traps(&component)?;
    let (ctx, io) = WasiCtxBuilder::new().build();
    let mut store = Store::new(
        &engine,
        Host {
            ctx,
            io,
            table: ResourceTable::new(),
        },
    );
    store.set_fuel(u64::MAX)?;
    store.set_epoch_deadline(u64::MAX);
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let guest = instance
        .get_export_index(&mut store, None, "golem:agent/guest@2.0.0")
        .context("guest export")?;
    let discover = instance
        .get_export_index(&mut store, Some(&guest), "discover-agent-types")
        .context("discover-agent-types export")?;
    let func = instance
        .get_func(&mut store, discover)
        .context("discover-agent-types function")?;
    let mut results = [Val::Bool(false)];
    func.call_async(&mut store, &[], &mut results).await?;
    Ok(results[0].clone())
}

#[test]
async fn builds_unrelated_apps_from_the_same_prepared_assets() -> Result<()> {
    let prepared = prepared()?;
    let sdk = python_sdk().join("src");

    for (module, source) in [
        ("first_app", "VALUE = 1\n"),
        (
            "second_app",
            "import json\n\nVALUE = json.dumps({'a': [1, 2]})\n",
        ),
    ] {
        let dir = tempfile::tempdir()?;
        write_app(dir.path(), module, source)?;
        let component = build(
            &prepared,
            &Application {
                sdk_dir: &sdk,
                source_dir: dir.path(),
                entry_modules: &[module],
            },
        )
        .await?;

        assert_eq!(
            discover_agent_types(&component).await?,
            Val::Result(Ok(Some(Box::new(Val::List(vec![]))))),
            "{module} declares no agents"
        );
    }
    Ok(())
}

#[test]
async fn import_errors_keep_the_original_traceback() -> Result<()> {
    let prepared = prepared()?;
    let dir = tempfile::tempdir()?;
    write_app(
        dir.path(),
        "broken",
        "def load():\n    raise ValueError('cannot load settings')\n\nload()\n",
    )?;

    let error = build(
        &prepared,
        &Application {
            sdk_dir: &python_sdk().join("src"),
            source_dir: dir.path(),
            entry_modules: &["broken"],
        },
    )
    .await
    .expect_err("an entry module that raises fails the build");

    let message = format!("{error:#}");
    assert!(
        message.contains("ValueError: cannot load settings"),
        "{message}"
    );
    assert!(
        message.contains("broken.py\", line 2, in load"),
        "{message}"
    );
    Ok(())
}

#[test]
async fn invalid_entry_modules_fail_before_imports() -> Result<()> {
    let prepared = prepared()?;
    let dir = tempfile::tempdir()?;
    // Importing `marker` writes a file into the application directory.
    write_app(dir.path(), "marker", "open('/app/imported', 'w').close()\n")?;
    let sdk = python_sdk().join("src");

    let error = build(
        &prepared,
        &Application {
            sdk_dir: &sdk,
            source_dir: dir.path(),
            entry_modules: &["marker", "not a module"],
        },
    )
    .await
    .expect_err("an invalid entry module fails the build");
    assert!(
        format!("{error:#}").contains("`not a module` is not a Python module name"),
        "{error:#}"
    );
    assert!(!dir.path().join("imported").exists());

    build(
        &prepared,
        &Application {
            sdk_dir: &sdk,
            source_dir: dir.path(),
            entry_modules: &["marker"],
        },
    )
    .await?;
    assert!(
        dir.path().join("imported").exists(),
        "a valid build imports the entry module"
    );
    Ok(())
}

#[test]
async fn identical_inputs_give_identical_components() -> Result<()> {
    let prepared = prepared()?;
    let sdk = python_sdk().join("src");
    let source = "import json\n\nVALUE = json.dumps({'a': [1, 2]})\n";

    let mut components = Vec::new();
    // Each build uses a new application directory, and the assembler unpacks
    // the prepared assets into new directories. The application directories
    // stay alive until both builds finish, so the second one cannot reuse
    // the host inode of the first.
    let mut dirs = Vec::new();
    for _ in 0..2 {
        let dir = tempfile::tempdir()?;
        write_app(dir.path(), "app", source)?;
        components.push(
            build(
                &prepared,
                &Application {
                    sdk_dir: &sdk,
                    source_dir: dir.path(),
                    entry_modules: &["app"],
                },
            )
            .await?,
        );
        dirs.push(dir);
    }
    let differing = components[0]
        .iter()
        .zip(&components[1])
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(components[0].len(), components[1].len());
    assert_eq!(differing, 0, "the two builds differ in {differing} bytes");
    Ok(())
}
