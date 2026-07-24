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

use crate::app::build::check::DependencyFixStep;
use crate::app::context::BuildContext;
use crate::app::edit;
use crate::fs;
use crate::model::GuestLanguage;
use crate::sdk_overrides::{GO_SDK_MODULE, SdkOverrides};

/// Reconcile each Go component's `go.mod` SDK dependency with the active SDK
/// overrides — mirrors the Rust `Cargo.toml` and TS `package.json` fix steps, so
/// a committed Go app picks up `GOLEM_GO_PATH`/`GOLEM_GO_VERSION` on build rather
/// than only at `golem new`.
///
/// Simpler than Rust: Go has no workspace concept, so each component is a
/// standalone module edited independently. `go.mod` sits exactly at the
/// component dir in both flat (`dir: ""` → app root) and multi-component layouts.
pub(super) fn plan_go_mod_fix_steps(
    ctx: &BuildContext<'_>,
    overrides: &SdkOverrides,
) -> anyhow::Result<Vec<DependencyFixStep>> {
    let version = overrides.go_sdk_dep();
    let replace_path = overrides.go_sdk_path.as_deref();

    let mut steps = Vec::new();
    for component_name in ctx.application_context().selected_component_names() {
        let component = ctx.application().component(component_name);
        if component.guess_language() != Some(GuestLanguage::Go) {
            continue;
        }

        let go_mod_path = component.component_dir().join("go.mod");
        if !go_mod_path.exists() {
            continue;
        }

        let original = fs::read_to_string(&go_mod_path)?;
        let new = edit::go_mod::reconcile_sdk_dependency(
            &original,
            GO_SDK_MODULE,
            &version,
            replace_path,
        );

        // A trailing-whitespace-only difference is not a semantic dependency
        // change — the reconciler always normalizes to a single trailing
        // newline, but a freshly generated go.mod can carry a trailing blank
        // line (e.g. an empty replace placeholder in version mode). Skipping it
        // avoids a spurious go.mod-change confirmation on the first build.
        if new.trim_end() != original.trim_end() {
            steps.push(DependencyFixStep {
                path: go_mod_path,
                current: original,
                new,
            });
        }
    }

    Ok(steps)
}
