// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

test_r::enable!();

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use test_r::test;

const AGENT_GUEST: &str = "golem:agent/guest@2.0.0";
const TOOL_GUEST: &str = "golem:tool/guest@0.1.0";
const TOOL_HOST: &str = "golem:tool/host@0.1.0";
const TOOL_MIDDLEWARE_GUEST: &str = "golem:tool/tool-middleware-guest@0.1.0";
const CONSTRUCTOR_DIAGNOSTIC: &str = "tool middleware `constructor` must be synchronous, infallible, zero-argument, and return the middleware implementation type (`fn() -> Self`)";
const CONSTRUCTOR_DIAGNOSTIC_SYMBOL: &str =
    "constructor_must_be_synchronous_infallible_zero_argument_and_return_self";

struct FixtureLockfile(PathBuf);

impl Drop for FixtureLockfile {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            panic!(
                "failed to remove fixture lockfile {}: {error}",
                self.0.display()
            );
        }
    }
}

#[test]
fn tool_middleware_cross_crate_components_and_compile_failures() {
    let fixture = fixture_root();
    let _lockfile = FixtureLockfile(fixture.join("Cargo.lock"));
    let target = target_dir();

    assert_feature_fixture_contract(&fixture, &target);
    check_fixture(&fixture, &target, "all-sdk-features-native");

    let pure = build_component(&fixture, &target, "pure-middleware-component");
    assert_component_contract(&component_wit(&pure), false, false, true, false);

    let ordinary = build_component(&fixture, &target, "ordinary-agentic-component");
    assert_component_contract(&component_wit(&ordinary), true, true, false, true);

    let combined = build_component(&fixture, &target, "combined-agentic-middleware-component");
    assert_component_contract(&component_wit(&combined), true, true, true, true);

    let all_wasi_features =
        build_component(&fixture, &target, "all-wasi-compatible-features-component");
    assert_component_contract(&component_wit(&all_wasi_features), true, true, true, true);

    for (binary, fragments) in [
        (
            "async-constructor",
            &[CONSTRUCTOR_DIAGNOSTIC, CONSTRUCTOR_DIAGNOSTIC_SYMBOL][..],
        ),
        (
            "fallible-constructor",
            &[CONSTRUCTOR_DIAGNOSTIC, CONSTRUCTOR_DIAGNOSTIC_SYMBOL][..],
        ),
        ("argument-constructor", &[CONSTRUCTOR_DIAGNOSTIC_SYMBOL][..]),
        (
            "wrong-return-constructor",
            &[CONSTRUCTOR_DIAGNOSTIC, CONSTRUCTOR_DIAGNOSTIC_SYMBOL][..],
        ),
        (
            "missing-annotation",
            &["__golem_tool_middleware_annotation"][..],
        ),
        (
            "generic-impl",
            &["requires a concrete, non-generic implementation"][..],
        ),
    ] {
        assert_compile_failure(&fixture, &target, binary, fragments);
    }
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tool-middleware")
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("golem-rust crate has an SDK workspace parent")
                .join("target")
        })
}

fn assert_feature_fixture_contract(fixture: &Path, target: &Path) {
    let output = cargo(fixture, target, ["metadata", "--format-version=1"]);
    assert_success(&output, "reading fixture workspace metadata");
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata emits JSON");
    let packages = metadata["packages"]
        .as_array()
        .expect("cargo metadata has a package array");
    let package = |name: &str| {
        packages
            .iter()
            .find(|package| package["name"] == name)
            .unwrap_or_else(|| panic!("cargo metadata has no `{name}` package"))
    };

    let sdk_features = package_features(package("golem-rust"));
    let native = package("all-sdk-features-native");
    let wasi = package("all-wasi-compatible-features-component");
    let native_features = dependency_features(native, "golem-rust");
    let wasi_features = dependency_features(wasi, "golem-rust");

    assert_eq!(
        native_features, sdk_features,
        "native all-feature fixture must explicitly enable every SDK feature"
    );
    let mut expected_wasi_features = sdk_features;
    assert!(
        expected_wasi_features.remove("mac_address"),
        "the SDK must retain the explicitly excluded `mac_address` feature"
    );
    assert_eq!(
        wasi_features, expected_wasi_features,
        "WASI all-feature fixture must differ from the SDK feature set only by `mac_address`"
    );
}

fn package_features(package: &serde_json::Value) -> BTreeSet<String> {
    package["features"]
        .as_object()
        .expect("cargo package metadata has a feature map")
        .keys()
        .cloned()
        .collect()
}

fn dependency_features(package: &serde_json::Value, dependency: &str) -> BTreeSet<String> {
    let dependency = package["dependencies"]
        .as_array()
        .expect("cargo package metadata has dependencies")
        .iter()
        .find(|candidate| candidate["name"] == dependency)
        .unwrap_or_else(|| {
            panic!(
                "fixture package `{}` does not depend on `{dependency}`",
                package["name"]
            )
        });
    assert_eq!(
        dependency["uses_default_features"], false,
        "fixture package `{}` must disable implicit default features",
        package["name"]
    );
    dependency["features"]
        .as_array()
        .expect("cargo dependency metadata has a feature array")
        .iter()
        .map(|feature| {
            feature
                .as_str()
                .expect("cargo dependency features are strings")
                .to_string()
        })
        .collect()
}

fn check_fixture(fixture: &Path, target: &Path, package: &str) {
    let output = cargo(fixture, target, ["check", "-p", package]);
    assert_success(&output, &format!("checking fixture package `{package}`"));
}

fn build_component(fixture: &Path, target: &Path, package: &str) -> PathBuf {
    let clean = cargo(
        fixture,
        target,
        ["clean", "-p", package, "--target", "wasm32-wasip2"],
    );
    assert_success(
        &clean,
        &format!("cleaning fixture package `{package}` before building"),
    );

    let output = cargo(
        fixture,
        target,
        [
            "build",
            "-p",
            package,
            "--target",
            "wasm32-wasip2",
            "--message-format=json-render-diagnostics",
        ],
    );
    assert_success(&output, &format!("building fixture package `{package}`"));

    let target_name = package.replace('-', "_");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter(|message| message["target"]["name"] == target_name)
        .flat_map(|message| message["filenames"].as_array().cloned().unwrap_or_default())
        .filter_map(|filename| filename.as_str().map(PathBuf::from))
        .find(|filename| {
            filename
                .extension()
                .is_some_and(|extension| extension == "wasm")
        })
        .unwrap_or_else(|| {
            panic!(
                "cargo did not report a WASM artifact for `{package}`:\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
}

fn component_wit(component: &Path) -> String {
    let output = Command::new("wasm-tools")
        .args(["component", "wit"])
        .arg(component)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to inspect fixture component {} with wasm-tools: {error}",
                component.display()
            )
        });
    assert_success(
        &output,
        &format!("inspecting fixture component `{}`", component.display()),
    );
    String::from_utf8(output.stdout).expect("wasm-tools emits UTF-8 WIT")
}

fn assert_component_contract(
    wit: &str,
    exports_agent: bool,
    exports_tool: bool,
    exports_middleware: bool,
    imports_tool_host: bool,
) {
    let (imports, exports) = root_world_interfaces(wit);
    let relevant = [AGENT_GUEST, TOOL_GUEST, TOOL_MIDDLEWARE_GUEST, TOOL_HOST];
    let mut actual_imports = imports
        .into_iter()
        .filter(|interface| relevant.contains(&interface.as_str()))
        .collect::<Vec<_>>();
    let mut actual_exports = exports
        .into_iter()
        .filter(|interface| relevant.contains(&interface.as_str()))
        .collect::<Vec<_>>();
    let mut expected_imports = [imports_tool_host.then_some(TOOL_HOST)]
        .into_iter()
        .flatten()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut expected_exports = [
        exports_agent.then_some(AGENT_GUEST),
        exports_tool.then_some(TOOL_GUEST),
        exports_middleware.then_some(TOOL_MIDDLEWARE_GUEST),
    ]
    .into_iter()
    .flatten()
    .map(str::to_string)
    .collect::<Vec<_>>();

    actual_imports.sort();
    actual_exports.sort();
    expected_imports.sort();
    expected_exports.sort();

    assert_eq!(
        actual_imports, expected_imports,
        "unexpected relevant root-world imports in component contract:\n{wit}"
    );
    assert_eq!(
        actual_exports, expected_exports,
        "unexpected relevant root-world exports in component contract:\n{wit}"
    );

    if !imports_tool_host {
        assert!(
            !wit.contains(TOOL_HOST),
            "component contract unexpectedly contains `{TOOL_HOST}`:\n{wit}"
        );
    }
}

fn root_world_interfaces(wit: &str) -> (Vec<String>, Vec<String>) {
    let root = wit
        .split_once("world root {")
        .map(|(_, root)| root)
        .unwrap_or_else(|| panic!("component contract has no `world root` declaration:\n{wit}"));
    let mut imports = Vec::new();
    let mut exports = Vec::new();

    for line in root.lines() {
        let line = line.trim();
        if line == "}" {
            return (imports, exports);
        }
        let (target, declaration) = if let Some(declaration) = line.strip_prefix("import ") {
            (&mut imports, declaration)
        } else if let Some(declaration) = line.strip_prefix("export ") {
            (&mut exports, declaration)
        } else {
            continue;
        };
        let interface = declaration.strip_suffix(';').unwrap_or_else(|| {
            panic!("root-world interface declaration is not semicolon-terminated: `{line}`")
        });
        target.push(interface.to_string());
    }

    panic!("component contract has no closing `world root` brace:\n{wit}")
}

fn assert_compile_failure(fixture: &Path, target: &Path, binary: &str, fragments: &[&str]) {
    let output = cargo(
        fixture,
        target,
        [
            "check",
            "--quiet",
            "-p",
            "middleware-compile-fail",
            "--bin",
            binary,
        ],
    );
    assert!(
        !output.status.success(),
        "compile-fail fixture `{binary}` unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    for fragment in fragments {
        assert!(
            stderr.contains(fragment),
            "compile-fail fixture `{binary}` did not contain `{fragment}`:\n{stderr}"
        );
    }
}

fn cargo<const N: usize>(fixture: &Path, target: &Path, arguments: [&str; N]) -> Output {
    Command::new("cargo")
        .args(arguments)
        .env("CARGO_TARGET_DIR", target)
        .current_dir(fixture)
        .output()
        .unwrap_or_else(|error| panic!("failed to run cargo in {}: {error}", fixture.display()))
}

fn assert_success(output: &Output, operation: &str) {
    assert!(
        output.status.success(),
        "failed while {operation}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
