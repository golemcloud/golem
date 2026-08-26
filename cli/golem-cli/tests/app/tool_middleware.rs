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
use std::path::{Path, PathBuf};
use test_r::test;
use wit_parser::WorldItem;
use wit_parser::decoding::DecodedWasm;

const APP_NAME: &str = "ts-tool-middleware-roles";
const ORDINARY_COMPONENT: &str = "ts-tool-middleware-roles:ordinary";
const MIDDLEWARE_COMPONENT: &str = "ts-tool-middleware-roles:middleware";

const AGENT_GUEST: &str = "golem:agent/guest@2.0.0";
const LOAD_SNAPSHOT: &str = "golem:api/load-snapshot@1.5.0";
const SAVE_SNAPSHOT: &str = "golem:api/save-snapshot@1.5.0";
const TOOL_GUEST: &str = "golem:tool/guest@0.1.0";
const TOOL_HOST: &str = "golem:tool/host@0.1.0";
const TOOL_MIDDLEWARE_GUEST: &str = "golem:tool/tool-middleware-guest@0.1.0";

#[test]
async fn test_ts_tool_middleware_component_roles() {
    let mut ctx = TestContext::new();
    let fixture = ctx.test_data_path_join(APP_NAME);
    fs_extra::dir::copy(fixture, ctx.cwd_path(), &fs_extra::dir::CopyOptions::new()).unwrap();
    ctx.cd(APP_NAME);

    let optimized = ctx.cli([flag::YES, cmd::BUILD, flag::FORCE_BUILD]).await;
    assert!(optimized.success_or_dump());
    assert_role_contracts(&ctx);
    for role in ["ordinary", "middleware", "combined"] {
        assert!(
            preinitialized_component(&ctx, role).is_file(),
            "optimized preset did not preinitialize the {role} component"
        );
    }

    std::fs::remove_dir_all(ctx.cwd_path_join("golem-temp")).unwrap();

    let quick = ctx
        .cli(["-P", "quick", flag::YES, cmd::BUILD, flag::FORCE_BUILD])
        .await;
    assert!(quick.success_or_dump());
    assert_role_contracts(&ctx);
    for role in ["ordinary", "middleware", "combined"] {
        assert!(
            dynamic_component(&ctx, role).is_file(),
            "quick preset did not inject the {role} component"
        );
        assert!(
            !preinitialized_component(&ctx, role).exists(),
            "quick preset unexpectedly preinitialized the {role} component"
        );
    }

    assert_wrong_role_diagnostics(&ctx).await;
}

fn assert_role_contracts(ctx: &TestContext) {
    assert_component_contract(
        &final_component(ctx, "ordinary"),
        &[AGENT_GUEST, LOAD_SNAPSHOT, SAVE_SNAPSHOT, TOOL_GUEST],
        true,
    );
    assert_component_contract(
        &final_component(ctx, "middleware"),
        &[TOOL_MIDDLEWARE_GUEST],
        false,
    );
    assert_component_contract(
        &final_component(ctx, "combined"),
        &[
            AGENT_GUEST,
            LOAD_SNAPSHOT,
            SAVE_SNAPSHOT,
            TOOL_GUEST,
            TOOL_MIDDLEWARE_GUEST,
        ],
        true,
    );
}

fn assert_component_contract(component: &Path, expected_exports: &[&str], expects_tool_host: bool) {
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

    let mut exports = world
        .exports
        .iter()
        .filter_map(|(key, item)| match item {
            WorldItem::Interface { .. } => Some(resolve.name_world_key(key)),
            WorldItem::Function(_) | WorldItem::Type { .. } => None,
        })
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
        "unexpected direct root interface exports in {}",
        component.display()
    );

    let direct_tool_host_imports = world
        .imports
        .iter()
        .filter(|(key, item)| {
            matches!(item, WorldItem::Interface { .. }) && resolve.name_world_key(key) == TOOL_HOST
        })
        .count();
    assert_eq!(
        direct_tool_host_imports,
        usize::from(expects_tool_host),
        "unexpected direct {TOOL_HOST} import count in {}",
        component.display()
    );
}

async fn assert_wrong_role_diagnostics(ctx: &TestContext) {
    std::fs::copy(
        ctx.cwd_path_join("combined/src/main.ts"),
        ctx.cwd_path_join("ordinary/src/main.ts"),
    )
    .unwrap();
    let ordinary = ctx
        .cli([flag::YES, cmd::BUILD, ORDINARY_COMPONENT, flag::FORCE_BUILD])
        .await;
    assert!(
        !ordinary.success(),
        "ordinary role mismatch unexpectedly built"
    );
    let ordinary_diagnostic =
        "defines tool middleware, but component template \"ts\" does not export tool middleware";
    assert!(
        ordinary.stdout_contains(ordinary_diagnostic)
            || ordinary.stderr_contains(ordinary_diagnostic),
        "ordinary role mismatch did not produce the expected diagnostic"
    );

    std::fs::copy(
        ctx.test_data_path_join(format!("{APP_NAME}/ordinary/src/main.ts")),
        ctx.cwd_path_join("middleware/src/main.ts"),
    )
    .unwrap();
    let middleware = ctx
        .cli([
            flag::YES,
            cmd::BUILD,
            MIDDLEWARE_COMPONENT,
            flag::FORCE_BUILD,
        ])
        .await;
    assert!(
        !middleware.success(),
        "pure role mismatch unexpectedly built"
    );
    let middleware_diagnostic = "component template \"ts-tool-middleware\" requires \"@golemcloud/golem-ts-sdk/middleware\"";
    assert!(
        middleware.stdout_contains(middleware_diagnostic)
            || middleware.stderr_contains(middleware_diagnostic),
        "pure role mismatch did not produce the expected diagnostic"
    );
}

fn final_component(ctx: &TestContext, role: &str) -> PathBuf {
    ctx.cwd_path_join(format!(
        "golem-temp/agents/ts_tool_middleware_roles_{role}.wasm"
    ))
}

fn dynamic_component(ctx: &TestContext, role: &str) -> PathBuf {
    ctx.cwd_path_join(format!(
        "golem-temp/agents/ts_tool_middleware_roles_{role}.dynamic.wasm"
    ))
}

fn preinitialized_component(ctx: &TestContext, role: &str) -> PathBuf {
    ctx.cwd_path_join(format!(
        "golem-temp/agents/ts_tool_middleware_roles_{role}.preinitialized.wasm"
    ))
}
