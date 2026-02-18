// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::app::context::BuildContext;
use crate::fs;
use crate::log::{log_action, LogColorize, LogIndent};

pub async fn link(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    log_action("Linking", "components");
    let _indent = LogIndent::new();

    for component_name in ctx.application_context().selected_component_names() {
        let component = ctx.application().component(component_name);
        let component_wasm = component.wasm();
        let linked_wasm = component.temp_linked_wasm();

        log_action(
            "Copying",
            format!(
                "{} to linked output",
                component_name.as_str().log_color_highlight(),
            ),
        );
        fs::copy(&component_wasm, &linked_wasm).map(|_| ())?;
    }

    Ok(())
}
