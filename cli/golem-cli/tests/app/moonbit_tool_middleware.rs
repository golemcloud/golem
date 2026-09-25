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

use super::{TestContext, cmd, flag};
use golem_cli::sdk_overrides::sdk_overrides;
use std::path::{Path, PathBuf};
use test_r::test;
use wit_parser::decoding::DecodedWasm;
use wit_parser::{Resolve, WorldItem, WorldKey};

const APP_NAME: &str = "moonbit-tool-middleware-roles";

/// Copies the fixture app into the test cwd and points its MoonBit SDK dependency
/// at the local SDK checkout.
fn setup_app(ctx: &mut TestContext) {
    let fixture = ctx.test_data_path_join(APP_NAME);
    fs_extra::dir::copy(fixture, ctx.cwd_path(), &fs_extra::dir::CopyOptions::new()).unwrap();
    ctx.cd(APP_NAME);

    let sdk_path = sdk_overrides()
        .unwrap()
        .moonbit_sdk_path
        .as_ref()
        .expect("MoonBit role integration requires the local SDK override");
    let module_path = ctx.cwd_path_join("moon.mod.json");
    let mut module: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&module_path).unwrap()).unwrap();
    module["deps"]["golemcloud/golem_sdk"] = serde_json::json!({ "path": sdk_path });
    std::fs::write(
        module_path,
        serde_json::to_string_pretty(&module).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
async fn test_moonbit_tool_middleware_component_roles() {
    let mut ctx = TestContext::new();
    setup_app(&mut ctx);

    let output = ctx.cli([flag::YES, cmd::BUILD, flag::FORCE_BUILD]).await;
    assert!(output.success_or_dump());

    for component in ["ordinary", "middleware", "combined"] {
        assert_default_package(&ctx.cwd_path_join(format!("{component}/moon.pkg")));
    }

    assert_component_contracts(&ctx, "debug");

    let output = ctx
        .cli([flag::YES, "-P", "release", cmd::BUILD, flag::FORCE_BUILD])
        .await;
    assert!(output.success_or_dump());

    assert_component_contracts(&ctx, "release");
}

/// A build with nothing to do must not touch the component wasms, otherwise the
/// `add-metadata` and `gen-bridge` steps re-run on every build. This exercises both
/// halves of that: `golem_sdk_tools` leaving unchanged generated sources alone, and
/// the `wasm-tools` steps declaring `sources`/`targets` so they can be skipped.
/// Deliberately built without `--force-build`, which bypasses all up-to-date checks.
#[test]
async fn test_moonbit_build_is_up_to_date_when_nothing_changed() {
    let mut ctx = TestContext::new();
    setup_app(&mut ctx);

    // Two builds to settle: after the first, sources and targets can share the same
    // filesystem timestamp, and the check requires targets to be strictly newer.
    for _ in 0..2 {
        let output = ctx.cli([flag::YES, cmd::BUILD]).await;
        assert!(output.success_or_dump());
    }

    let watched = [
        ordinary_component(&ctx, "debug"),
        middleware_component(&ctx, "debug"),
        combined_component(&ctx, "debug"),
        ctx.cwd_path_join("ordinary/golem_agents.mbt"),
        ctx.cwd_path_join("ordinary/golem_reexports.mbt"),
        ctx.cwd_path_join("ordinary/moon.pkg"),
    ];
    let before = watched.iter().map(modified_at).collect::<Vec<_>>();

    let output = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(output.success_or_dump());

    for (path, before) in watched.iter().zip(before) {
        assert_eq!(
            modified_at(path),
            before,
            "{} was rewritten by an up-to-date build",
            path.display()
        );
    }
}

fn modified_at(path: &PathBuf) -> std::time::SystemTime {
    std::fs::metadata(path)
        .unwrap_or_else(|err| panic!("failed to stat {}: {err}", path.display()))
        .modified()
        .unwrap()
}

fn assert_component_contracts(ctx: &TestContext, profile: &str) {
    let expected_imports = [
        "interface:golem:agent/common@2.0.0",
        "interface:golem:agent/host@2.0.0",
        "interface:golem:api/host@1.5.0",
        "interface:golem:core/types@2.0.0",
        "interface:golem:tool/common@0.1.0",
        "interface:golem:tool/host@0.1.0",
        "interface:golem:tool/streams@0.1.0",
        "interface:golem:tool/underlying@0.1.0",
        "interface:wasi:cli/environment@0.3.0",
        "interface:wasi:clocks/types@0.3.0",
        "interface:wasi:logging/logging",
    ];
    let expected_exports = [
        "interface:golem:agent/guest@2.0.0",
        "interface:golem:api/load-snapshot@1.5.0",
        "interface:golem:api/save-snapshot@1.5.0",
        "interface:golem:tool/guest@0.1.0",
        "interface:golem:tool/tool-middleware-guest@0.1.0",
    ];
    for component in [
        ordinary_component(ctx, profile),
        middleware_component(ctx, profile),
        combined_component(ctx, profile),
    ] {
        assert_component_contract(&component, &expected_imports, &expected_exports);
    }
}

fn assert_default_package(path: &Path) {
    let package = std::fs::read_to_string(path).unwrap();
    assert!(
        package.contains("/gen\" @gen"),
        "{} does not import the default generated package:\n{package}",
        path.display()
    );
}

fn assert_component_contract(
    component: &Path,
    expected_imports: &[&str],
    expected_exports: &[&str],
) {
    let bytes = std::fs::read(component).unwrap();
    let decoded = wit_parser::decoding::decode(&bytes).unwrap();
    let (resolve, world_id) = match decoded {
        DecodedWasm::Component(resolve, world_id) => (resolve, world_id),
        DecodedWasm::WitPackage(_, _) => {
            panic!(
                "{} decoded as a WIT package, not a component",
                component.display()
            )
        }
    };
    let world = &resolve.worlds[world_id];

    let mut imports = world
        .imports
        .iter()
        .map(|(key, item)| normalized_world_item(&resolve, key, item))
        .collect::<Vec<_>>();
    imports.sort();
    let mut expected_imports = expected_imports
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    expected_imports.sort();
    assert_eq!(
        imports,
        expected_imports,
        "unexpected direct imports in {}",
        component.display()
    );

    let mut exports = world
        .exports
        .iter()
        .map(|(key, item)| normalized_world_item(&resolve, key, item))
        .collect::<Vec<_>>();
    exports.sort();

    let mut expected_exports = expected_exports
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    expected_exports.sort();
    assert_eq!(
        exports,
        expected_exports,
        "unexpected direct exports in {}",
        component.display()
    );
}

fn normalized_world_item(resolve: &Resolve, key: &WorldKey, item: &WorldItem) -> String {
    let kind = match item {
        WorldItem::Interface { .. } => "interface",
        WorldItem::Function(_) => "function",
        WorldItem::Type { .. } => "type",
    };
    format!("{kind}:{}", resolve.name_world_key(key))
}

fn ordinary_component(ctx: &TestContext, profile: &str) -> PathBuf {
    ctx.cwd_path_join(format!(
        "_build/wasm/{profile}/moonbit_tool_middleware_roles_ordinary.agent.wasm"
    ))
}

fn middleware_component(ctx: &TestContext, profile: &str) -> PathBuf {
    ctx.cwd_path_join(format!(
        "_build/wasm/{profile}/moonbit_tool_middleware_roles_middleware.agent.wasm"
    ))
}

fn combined_component(ctx: &TestContext, profile: &str) -> PathBuf {
    ctx.cwd_path_join(format!(
        "_build/wasm/{profile}/moonbit_tool_middleware_roles_combined.agent.wasm"
    ))
}
