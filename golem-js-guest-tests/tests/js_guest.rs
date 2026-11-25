// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

use anyhow::{Context, ensure};
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::javascript::{JS_COMPILATION_CACHE_MAX_BYTES, preinitialize_js};
use golem_common::model::agent::extraction::extract_agent_type_schemas;
use std::path::{Path, PathBuf};
use test_r::{test, timeout};

test_r::enable!();

async fn smoke_test(guest: &Path, bundle: &Path, expected: &str) -> anyhow::Result<()> {
    ensure!(
        std::fs::metadata(guest)
            .with_context(|| format!(
                "Missing JS guest {}; build its SDK agent template first",
                guest.display()
            ))?
            .len()
            > 0,
        "Empty JS guest: {}",
        guest.display()
    );
    let source = std::fs::read_to_string(bundle).with_context(|| {
        format!(
            "Failed to read JS bundle {}; run cargo make prepare-js-guest-tests",
            bundle.display()
        )
    })?;
    let directory = tempfile::tempdir()?;
    let injected = directory.path().join("injected.wasm");
    let initialized = directory.path().join("initialized.wasm");
    wasm_rquickjs::inject_js_into_component(
        Utf8Path::from_path(guest).context("Non-UTF-8 guest path")?,
        Utf8Path::from_path(&injected).context("Non-UTF-8 temporary path")?,
        &[&source],
    )?;
    let quick_agents = extract_agent_type_schemas(&injected, true, false).await?;
    assert_eq!(quick_agents.len(), 1);
    assert_eq!(quick_agents[0].type_name.0, "JsGuestSmoke");
    assert!(
        quick_agents[0]
            .description
            .starts_with(expected.trim_end_matches('0'))
    );
    assert_ne!(quick_agents[0].description, expected);
    golem_common::javascript::preinitialize_js(
        Utf8Path::from_path(guest).context("Non-UTF-8 guest path")?,
        Utf8Path::from_path(&initialized).context("Non-UTF-8 temporary path")?,
        source,
        None,
    )
    .await?;

    // Exercise the real discovery export with the production engine configuration.
    let agents = extract_agent_type_schemas(&initialized, true, false).await?;
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].type_name.0, "JsGuestSmoke");
    assert_eq!(agents[0].description, expected);
    Ok(())
}

fn bundle(sdk: &str) -> anyhow::Result<PathBuf> {
    let directory = std::env::var_os("GOLEM_JS_GUEST_BUNDLE_DIR").context(
        "GOLEM_JS_GUEST_BUNDLE_DIR is unset; run cargo make js-guest-tests to prepare and test fresh bundles",
    )?;
    Ok(PathBuf::from(directory).join(sdk).join("smoke/smoke.mjs"))
}

#[test]
#[timeout("120s")]
async fn typescript_guest() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    smoke_test(
        &root.join("sdks/ts/packages/golem-ts-sdk/wasm/agent_guest.wasm"),
        &bundle("ts")?,
        "TS initialized value: 41, clock: 0",
    )
    .await
}

#[test]
#[timeout("120s")]
async fn effect_guest() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    smoke_test(
        &root.join("sdks/effect/wasm/agent_guest.wasm"),
        &bundle("effect")?,
        "Effect initialized value: 29, clock: 0",
    )
    .await
}

#[test]
#[timeout("300s")]
async fn typescript_compilation_cache() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let guest = Utf8PathBuf::from_path_buf(
        root.join("sdks/ts/packages/golem-ts-sdk/wasm/agent_guest.wasm"),
    )
    .unwrap();
    let source = std::fs::read_to_string(bundle("ts")?)?;
    let edited = source.replace("TS initialized value:", "Edited TS initialized value:");
    assert_ne!(
        source, edited,
        "fixture must contain the description being edited"
    );
    let dir = tempfile::tempdir()?;
    let dir = Utf8Path::from_path(dir.path()).unwrap();
    let cache = dir.join("native-cache");
    let cold = dir.join("cold.wasm");
    let warm = dir.join("warm.wasm");
    let uncached = dir.join("uncached.wasm");
    let edited_cached = dir.join("edited-cached.wasm");
    let edited_uncached = dir.join("edited-uncached.wasm");

    let stats = preinitialize_js(&guest, &cold, source.clone(), Some(cache.as_std_path()))
        .await?
        .compilation_cache
        .unwrap();
    assert!(stats.misses > 0);
    let stats = preinitialize_js(&guest, &warm, source.clone(), Some(cache.as_std_path()))
        .await?
        .compilation_cache
        .unwrap();
    assert!(stats.hits > 0);
    assert_eq!(stats.misses, 0);
    assert!(
        preinitialize_js(&guest, &uncached, source.clone(), None)
            .await?
            .compilation_cache
            .is_none()
    );
    assert_eq!(std::fs::read(&cold)?, std::fs::read(&warm)?);
    assert_eq!(std::fs::read(&cold)?, std::fs::read(&uncached)?);

    let stats = preinitialize_js(
        &guest,
        &edited_cached,
        edited.clone(),
        Some(cache.as_std_path()),
    )
    .await?
    .compilation_cache
    .unwrap();
    assert!(
        stats.hits > 0,
        "a JS edit must reuse compiled template code"
    );
    assert_eq!(stats.misses, 0);
    preinitialize_js(&guest, &edited_uncached, edited.clone(), None).await?;
    assert_eq!(
        std::fs::read(&edited_cached)?,
        std::fs::read(&edited_uncached)?
    );
    let agents = extract_agent_type_schemas(edited_cached.as_std_path(), true, false).await?;
    assert_eq!(
        agents[0].description,
        "Edited TS initialized value: 41, clock: 0"
    );

    let parallel_a = dir.join("parallel-a.wasm");
    let parallel_b = dir.join("parallel-b.wasm");
    let (a, b) = tokio::join!(
        preinitialize_js(
            &guest,
            &parallel_a,
            source.clone(),
            Some(cache.as_std_path())
        ),
        preinitialize_js(&guest, &parallel_b, edited, Some(cache.as_std_path())),
    );
    a?;
    b?;
    assert_eq!(std::fs::read(&parallel_a)?, std::fs::read(&uncached)?);
    assert_eq!(
        std::fs::read(&parallel_b)?,
        std::fs::read(&edited_uncached)?
    );

    let mut engine_config =
        golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache();
    engine_config.cranelift_opt_level(wasmtime::OptLevel::None);
    let mut cache_config = wasmtime::CacheConfig::new();
    cache_config.with_directory(cache.as_std_path());
    let sources = [source.clone()];
    let other_engine = dir.join("other-engine.wasm");
    let stats = wasm_rquickjs::optimize_component_with_options(
        &guest,
        &other_engine,
        "wizer-initialize",
        &wasm_rquickjs::OptimizeOptions {
            hostcall_fuel: Some(512 * 1024 * 1024),
            sources: Some(&sources),
            engine_config: Some(&engine_config),
            cache_config: Some(&cache_config),
        },
    )
    .await?
    .compilation_cache
    .unwrap();
    assert!(stats.misses > 0, "different engine configuration must miss");

    // A damaged artifact is disposable: the build must recompile, not fail.
    for entry in walkdir::WalkDir::new(&cache) {
        let entry = entry?;
        if entry.file_type().is_file() {
            std::fs::write(entry.path(), [])?;
        }
    }
    let recovered = dir.join("recovered.wasm");
    let stats = preinitialize_js(&guest, &recovered, source, Some(cache.as_std_path()))
        .await?
        .compilation_cache
        .unwrap();
    assert!(stats.misses > 0);
    assert_eq!(std::fs::read(&recovered)?, std::fs::read(&uncached)?);
    Ok(())
}

#[test]
#[timeout("600s")]
async fn compilation_cache_stays_bounded_after_twenty_guests() -> anyhow::Result<()> {
    use wasm_encoder::ComponentSection;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let template = std::fs::read(root.join("sdks/ts/packages/golem-ts-sdk/wasm/agent_guest.wasm"))?;
    let source = std::fs::read_to_string(bundle("ts")?)?;
    let directory = tempfile::tempdir()?;
    let directory = Utf8Path::from_path(directory.path()).unwrap();
    let cache = directory.join("native-cache");
    let input = directory.join("template.wasm");
    let output = directory.join("output.wasm");
    for i in 0..20u8 {
        let mut guest = template.clone();
        wasm_encoder::CustomSection {
            name: "cache-case".into(),
            data: vec![i].into(),
        }
        .append_to_component(&mut guest);
        std::fs::write(&input, guest)?;
        let stats = preinitialize_js(&input, &output, source.clone(), Some(cache.as_std_path()))
            .await?
            .compilation_cache
            .unwrap();
        assert!(stats.misses > 0, "each distinct guest must compile");
        let size: u64 = walkdir::WalkDir::new(&cache)
            .into_iter()
            .map(|entry| {
                let entry = entry?;
                Ok::<_, anyhow::Error>(if entry.file_type().is_file() {
                    entry.metadata()?.len()
                } else {
                    0
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .sum();
        assert!(
            size <= JS_COMPILATION_CACHE_MAX_BYTES,
            "cache grew to {size} bytes after guest {i}"
        );
    }
    Ok(())
}
