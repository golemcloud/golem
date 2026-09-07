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

use super::RunArgs;
use crate::model::app_raw::Application;
use clap::Parser;
use test_r::test;

#[derive(Parser)]
struct ServerCommand {
    #[command(flatten)]
    args: RunArgs,
}

#[test]
fn local_server_system_memory_override_parses_sizes_consistently() {
    for (value, expected) in [
        ("1mb", 1_000_000),
        ("1MB", 1_000_000),
        ("1 MiB", 1_048_576),
        ("2GiB", 2_147_483_648),
    ] {
        let flag = ServerCommand::try_parse_from(["server", "--system-memory-override", value])
            .unwrap()
            .args;
        let env = RunArgs::default()
            .with_env_overrides_from(|name| {
                assert_eq!(name, "GOLEM_LOCAL_SERVER_SYSTEM_MEMORY_OVERRIDE");
                Ok(value.to_string())
            })
            .unwrap();
        let manifest = Application::from_yaml_str(&format!(
            "app: test-app\nlocalServer:\n  systemMemoryOverride: '{value}'\n"
        ))
        .unwrap()
        .local_server
        .unwrap();
        assert_eq!(flag.system_memory_override.unwrap().get(), expected);
        assert_eq!(env.system_memory_override, flag.system_memory_override);
        assert_eq!(manifest.system_memory_override, flag.system_memory_override);
    }
}

#[test]
fn local_server_system_memory_override_env_precedence_and_errors() {
    let args = ServerCommand::try_parse_from(["server", "--system-memory-override=2GiB"])
        .unwrap()
        .args
        .with_env_overrides_from(|_| panic!("flag must bypass environment lookup"))
        .unwrap();
    assert_eq!(args.system_memory_override.unwrap().get(), 2_147_483_648);
    let args = RunArgs::default()
        .with_env_overrides_from(|_| Err(std::env::VarError::NotPresent))
        .unwrap();
    assert_eq!(args.system_memory_override, None);
    let error = RunArgs::default()
        .with_env_overrides_from(|_| Ok("invalid".into()))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("GOLEM_LOCAL_SERVER_SYSTEM_MEMORY_OVERRIDE")
    );
}
