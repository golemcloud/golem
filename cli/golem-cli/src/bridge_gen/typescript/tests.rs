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

use crate::bridge_gen::typescript::TypeScriptBridgeGenerator;
use crate::bridge_gen::BridgeGenerator;
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::model::agent::AgentType;
use test_r::test;

// TODO: write real tests
#[test]
fn playground1() {
    let agent_type =
        super::super::super::model::agent::test::single_agent_wrapper_types()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type, &target_dir);
}

#[test]
fn playground2() {
    let agent_type =
        super::super::super::model::agent::test::multi_agent_wrapper_2_types()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen2");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type, &target_dir);
}

fn generate_and_compile(agent_type: AgentType, target_dir: &Utf8Path) {
    let gen = TypeScriptBridgeGenerator::new(agent_type, target_dir);
    gen.generate().unwrap();

    let status = std::process::Command::new("npm")
        .arg("install")
        .current_dir(target_dir.as_std_path())
        .status()
        .expect("failed to run `npm install`");
    assert!(status.success(), "`npm install` failed: {:?}", status);

    let status = std::process::Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(target_dir.as_std_path())
        .status()
        .expect("failed to run `npm run build`");
    assert!(status.success(), "`npm run build` failed: {:?}", status);

    let generated = collect_js_and_d_ts(target_dir.as_std_path());
    assert!(
        !generated.is_empty(),
        "no .js or .d.ts files generated in `{}`",
        target_dir
    );
}

fn collect_js_and_d_ts(dir: &std::path::Path) -> Vec<Utf8PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    let mut result = vec![];
    while let Some(p) = stack.pop() {
        if let Ok(md) = std::fs::metadata(&p) {
            if md.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    for e in rd.flatten() {
                        stack.push(e.path());
                    }
                }
            } else if md.is_file() {
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    if name.ends_with(".js") || name.ends_with(".d.ts") {
                        result.push(Utf8PathBuf::from_path_buf(p.to_path_buf()).unwrap());
                    }
                }
            }
        }
    }
    result
}
