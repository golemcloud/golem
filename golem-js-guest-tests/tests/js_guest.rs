// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

use anyhow::{Context, ensure};
use camino::Utf8Path;
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
    golem_common::javascript::preinitialize_js(
        Utf8Path::from_path(&injected).context("Non-UTF-8 temporary path")?,
        Utf8Path::from_path(&initialized).context("Non-UTF-8 temporary path")?,
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
