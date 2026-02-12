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

use evcxr::CommandContext;

pub fn run_repl() -> anyhow::Result<()> {
    evcxr::runtime_hook();
    let (mut context, outputs) = CommandContext::new()?;
    context.eval("let mut s = String::new();")?;
    context.eval(r#"s.push_str("Hello, ");"#)?;
    context.eval(r#"s.push_str("World!");"#)?;
    context.eval(r#"println!("{}", s);"#)?;

    if let Ok(line) = outputs.stdout.recv() {
        println!("{line}");
    }

    Ok(())
}