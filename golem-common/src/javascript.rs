// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

use camino::Utf8Path;

/// Preinitializes a JavaScript component with Golem's build-time export and fuel budget.
pub async fn preinitialize_js(input: &Utf8Path, output: &Utf8Path) -> anyhow::Result<()> {
    wasm_rquickjs::optimize_component_with_options(
        input,
        output,
        "wizer-initialize",
        &wasm_rquickjs::OptimizeOptions {
            hostcall_fuel: Some(512 * 1024 * 1024),
        },
    )
    .await
}
