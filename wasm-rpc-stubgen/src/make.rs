// Copyright 2024-2025 Golem Cloud
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

use crate::log::{log_action, log_warn_action, LogColorize};
use crate::{cargo, fs, GenerateArgs, WasmRpcOverride};
use heck::ToSnakeCase;
use std::path::Path;
use std::process::Command;
use toml::map::Map;
use toml::Value;

struct CargoMake {
    config: CargoMakeConfig,
    tasks: Vec<Task>,
    other: Map<String, Value>,
}

impl CargoMake {
    pub fn new() -> Self {
        Self {
            config: CargoMakeConfig {
                default_to_workspace: false,
                unknown: Map::default(),
            },
            tasks: Vec::new(),
            other: Map::default(),
        }
    }

    pub fn load(raw: &str) -> anyhow::Result<Self> {
        let toml: Map<String, Value> = toml::from_str(raw)?;

        let mut config = CargoMakeConfig::default();
        let mut tasks = Vec::new();
        let mut other = Map::default();

        for (name, value) in toml {
            if name == "config" {
                config = CargoMakeConfig::from_value(value)?;
            } else if name == "tasks" {
                if let Value::Table(tasks_table) = value {
                    for (name, value) in tasks_table {
                        tasks.push(Task::Unknown {
                            name,
                            contents: value,
                        });
                    }
                } else {
                    return Err(anyhow::anyhow!("Expected table for [tasks]"));
                }
            } else {
                other.insert(name, value);
            }
        }

        Ok(Self {
            config,
            tasks,
            other,
        })
    }

    pub fn add_task(&mut self, task: Task) {
        self.tasks.retain(|t| t.name() != task.name());
        self.tasks.push(task);
    }

    pub fn to_string(&self) -> anyhow::Result<String> {
        let mut root = Map::default();

        root.insert("config".to_string(), self.config.to_value());

        let mut tasks = Map::default();
        for task in &self.tasks {
            tasks.insert(task.name(), task.to_value());
        }

        root.insert("tasks".to_string(), Value::Table(tasks));
        root.extend(self.other.clone());

        let result = toml::to_string(&root)?;
        Ok(result)
    }
}

#[derive(Default)]
struct CargoMakeConfig {
    default_to_workspace: bool,
    unknown: Map<String, Value>,
}

impl CargoMakeConfig {
    pub fn from_value(value: Value) -> anyhow::Result<Self> {
        if let Value::Table(table) = value {
            let default_to_workspace = table
                .get("default_to_workspace")
                .map_or(false, |v| v.as_bool().unwrap_or(false));
            let mut unknown = table.clone();
            unknown.retain(|k, _| k != "default_to_workspace");
            Ok(Self {
                default_to_workspace,
                unknown,
            })
        } else {
            Err(anyhow::anyhow!("Expected table for [config]"))
        }
    }

    pub fn to_value(&self) -> Value {
        let mut value = Map::default();
        for (k, v) in &self.unknown {
            value.insert(k.clone(), v.clone());
        }
        value.insert(
            "default_to_workspace".to_string(),
            Value::Boolean(self.default_to_workspace),
        );
        Value::Table(value)
    }
}

enum Task {
    Unknown {
        name: String,
        contents: Value,
    },
    Default,
    Clean,
    Build {
        release: bool,
    },
    Test,
    GenerateStub {
        target: String,
        wasm_rpc_override: WasmRpcOverride,
        stubgen_command: String,
        stubgen_prefix: Vec<String>,
    },
    AddStubDependency {
        target: String,
        caller: String,
        stubgen_command: String,
        stubgen_prefix: Vec<String>,
    },
    RegenerateStubs {
        dependencies: Vec<String>,
    },
    Compose {
        release: bool,
        caller: String,
        targets: Vec<String>,
        stubgen_command: String,
        stubgen_prefix: Vec<String>,
    },
    PostBuild {
        release: bool,
        tasks: Vec<String>,
    },
    BuildFlow {
        release: bool,
    },
}

impl Task {
    pub fn name(&self) -> String {
        match self {
            Task::Unknown { name, .. } => name.clone(),
            Task::Default => "default".to_string(),
            Task::Clean => "clean".to_string(),
            Task::Build { release: false } => "build".to_string(),
            Task::Build { release: true } => "build-release".to_string(),
            Task::Test => "test".to_string(),
            Task::GenerateStub { target, .. } => format!("generate-{}-stub", target),
            Task::AddStubDependency { target, caller, .. } => {
                format!("add-stub-dependency-{}-{}", target, caller)
            }
            Task::RegenerateStubs { .. } => "regenerate-stubs".to_string(),
            Task::Compose {
                release: false,
                caller,
                ..
            } => format!("compose-{}", caller),
            Task::Compose {
                release: true,
                caller,
                ..
            } => format!("compose-release-{}", caller),
            Task::PostBuild { release: false, .. } => "post-build".to_string(),
            Task::PostBuild { release: true, .. } => "post-build-release".to_string(),
            Task::BuildFlow { release: false } => "build-flow".to_string(),
            Task::BuildFlow { release: true } => "release-build-flow".to_string(),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Task::Unknown { contents, .. } => contents.clone(),
            Task::Default => {
                let mut value = Map::default();
                value.insert("alias".to_string(), Value::String("build".to_string()));
                Value::Table(value)
            }
            Task::Clean => {
                let mut clean = Map::default();
                clean.insert(
                    "command".to_string(),
                    Value::String("cargo-component".to_string()),
                );
                clean.insert(
                    "args".to_string(),
                    Value::Array(vec![Value::String("clean".to_string())]),
                );
                Value::Table(clean)
            }
            Task::Build { release } => {
                let mut build = Map::default();
                build.insert(
                    "command".to_string(),
                    Value::String("cargo-component".to_string()),
                );
                let mut args = vec![Value::String("build".to_string())];
                if *release {
                    args.push(Value::String("--release".to_string()));
                }
                build.insert("args".to_string(), Value::Array(args));
                build.insert(
                    "dependencies".to_string(),
                    Value::Array(vec![
                        Value::String("clean".to_string()),
                        Value::String("regenerate-stubs".to_string()),
                    ]),
                );
                Value::Table(build)
            }
            Task::Test => {
                let mut test = Map::default();
                test.insert(
                    "command".to_string(),
                    Value::String("cargo-component".to_string()),
                );
                test.insert(
                    "args".to_string(),
                    Value::Array(vec![Value::String("test".to_string())]),
                );
                test.insert(
                    "dependencies".to_string(),
                    Value::Array(vec![Value::String("clean".to_string())]),
                );
                Value::Table(test)
            }
            Task::GenerateStub {
                target,
                wasm_rpc_override,
                stubgen_command,
                stubgen_prefix,
            } => {
                let mut generate_stub = Map::default();
                generate_stub.insert("cwd".to_string(), Value::String(".".to_string()));
                generate_stub.insert(
                    "command".to_string(),
                    Value::String(stubgen_command.to_string()),
                );
                let mut args = Vec::new();
                args.extend(stubgen_prefix.iter().map(|s| Value::String(s.to_string())));
                args.push(Value::String("generate".to_string()));
                args.push(Value::String("-s".to_string()));
                args.push(Value::String(format!("{}/wit", target)));
                args.push(Value::String("-d".to_string()));
                args.push(Value::String(format!("{}-stub", target)));
                if let Some(wasm_rpc_path_override) =
                    wasm_rpc_override.wasm_rpc_path_override.as_ref()
                {
                    args.push(Value::String("--wasm-rpc-path-override".to_string()));
                    args.push(Value::String(wasm_rpc_path_override.to_string()));
                }
                if let Some(wasm_rpc_version_override) =
                    wasm_rpc_override.wasm_rpc_version_override.as_ref()
                {
                    args.push(Value::String("--wasm-rpc-version-override".to_string()));
                    args.push(Value::String(wasm_rpc_version_override.to_string()));
                }
                generate_stub.insert("args".to_string(), Value::Array(args));

                Value::Table(generate_stub)
            }
            Task::AddStubDependency {
                target,
                caller,
                stubgen_command,
                stubgen_prefix,
            } => {
                let mut add_stub_dependency = Map::default();
                add_stub_dependency.insert("cwd".to_string(), Value::String(".".to_string()));
                add_stub_dependency.insert(
                    "command".to_string(),
                    Value::String(stubgen_command.to_string()),
                );

                let mut args = Vec::new();
                args.extend(stubgen_prefix.iter().map(|s| Value::String(s.to_string())));
                args.extend(vec![
                    Value::String("add-stub-dependency".to_string()),
                    Value::String("--stub-wit-root".to_string()),
                    Value::String(format!("{}-stub/wit", target)),
                    Value::String("--dest-wit-root".to_string()),
                    Value::String(format!("{}/wit", caller)),
                    Value::String("--overwrite".to_string()),
                    Value::String("--update-cargo-toml".to_string()),
                ]);

                add_stub_dependency.insert("args".to_string(), Value::Array(args));
                add_stub_dependency.insert(
                    "dependencies".to_string(),
                    Value::Array(vec![Value::String(format!("generate-{}-stub", target))]),
                );

                Value::Table(add_stub_dependency)
            }
            Task::RegenerateStubs { dependencies } => {
                let mut regenerate_stubs = Map::default();
                regenerate_stubs.insert(
                    "dependencies".to_string(),
                    Value::Array(
                        dependencies
                            .iter()
                            .map(|n| Value::String(n.to_string()))
                            .collect::<Vec<_>>(),
                    ),
                );

                Value::Table(regenerate_stubs)
            }
            Task::Compose {
                release,
                caller,
                targets,
                stubgen_command,
                stubgen_prefix,
            } => {
                let debug_or_release = if *release { "release" } else { "debug" };

                let mut compose = Map::default();
                compose.insert("cwd".to_string(), Value::String(".".to_string()));
                compose.insert(
                    "command".to_string(),
                    Value::String(stubgen_command.to_string()),
                );
                let mut args = Vec::new();
                args.extend(stubgen_prefix.iter().map(|s| Value::String(s.to_string())));
                args.push(Value::String("compose".to_string()));
                args.push(Value::String("--source-wasm".to_string()));
                args.push(Value::String(format!(
                    "target/wasm32-wasi/{debug_or_release}/{}.wasm",
                    caller.to_snake_case()
                )));
                for target in targets {
                    args.push(Value::String("--stub-wasm".to_string()));
                    args.push(Value::String(format!(
                        "target/wasm32-wasi/{debug_or_release}/{}_stub.wasm",
                        target.to_snake_case()
                    )));
                }
                args.push(Value::String("--dest-wasm".to_string()));
                args.push(Value::String(format!(
                    "target/wasm32-wasi/{debug_or_release}/{}_composed.wasm",
                    caller.to_snake_case()
                )));

                compose.insert("args".to_string(), Value::Array(args));

                Value::Table(compose)
            }
            Task::PostBuild { tasks, .. } => {
                let mut post_build = Map::default();
                post_build.insert(
                    "dependencies".to_string(),
                    Value::Array(
                        tasks
                            .iter()
                            .map(|n| Value::String(n.to_string()))
                            .collect::<Vec<_>>(),
                    ),
                );

                Value::Table(post_build)
            }
            Task::BuildFlow { release } => {
                let mut build_flow = Map::default();

                let dependencies = if *release {
                    Value::Array(vec![
                        Value::String("build-release".to_string()),
                        Value::String("post-build-release".to_string()),
                    ])
                } else {
                    Value::Array(vec![
                        Value::String("build".to_string()),
                        Value::String("post-build".to_string()),
                    ])
                };

                build_flow.insert("dependencies".to_string(), dependencies);

                Value::Table(build_flow)
            }
        }
    }
}

pub fn initialize_workspace(
    targets: &[String],
    callers: &[String],
    wasm_rpc_override: WasmRpcOverride,
    stubgen_command: &str,
    stubgen_prefix: &[&str],
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let workspace_cargo = cwd.join("Cargo.toml");
    if cargo::is_cargo_workspace_toml(&workspace_cargo)? {
        let makefile_path = cwd.join("Makefile.toml");
        if makefile_path.exists() && makefile_path.is_file() {
            let mut makefile = read_makefile(&makefile_path)?;
            generate_makefile(
                &mut makefile,
                targets,
                callers,
                wasm_rpc_override.clone(),
                stubgen_command,
                stubgen_prefix,
            );
            let makefile = makefile.to_string()?;
            log_warn_action(
                "Overwriting",
                format!(
                    "cargo-make Makefile {:?}",
                    makefile_path.log_color_highlight()
                ),
            );
            fs::write(makefile_path, makefile)?;
        } else if has_cargo_make() {
            let mut makefile = CargoMake::new();
            generate_makefile(
                &mut makefile,
                targets,
                callers,
                wasm_rpc_override.clone(),
                stubgen_command,
                stubgen_prefix,
            );
            let makefile = makefile.to_string()?;
            log_action(
                "Writing",
                format!(
                    "cargo-make Makefile to {:?}",
                    makefile_path.log_color_highlight()
                ),
            );
            fs::write(makefile_path, makefile)?;
        } else {
            return Err(anyhow::anyhow!(
                "cargo-make is not installed. Please install it with `cargo install cargo-make`"
            ));
        }

        let mut new_members = Vec::new();
        for target in targets {
            log_action(
                "Generating",
                format!("initial stub for {}", target.log_color_highlight()),
            );

            let stub_name = format!("{target}-stub");
            crate::generate(GenerateArgs {
                source_wit_root: cwd.join(format!("{target}/wit")),
                dest_crate_root: cwd.join(stub_name.clone()),
                world: None,
                stub_crate_version: "0.0.1".to_string(),
                wasm_rpc_override: wasm_rpc_override.clone(),
                always_inline_types: false,
            })?;

            new_members.push(stub_name);
        }

        cargo::add_workspace_members(&workspace_cargo, &new_members)?;

        Ok(())
    } else {
        Err(anyhow::anyhow!("Not in a cargo workspace"))
    }
}

fn read_makefile(path: &Path) -> anyhow::Result<CargoMake> {
    let raw = fs::read_to_string(path)?;
    CargoMake::load(&raw)
}

fn generate_makefile(
    makefile: &mut CargoMake,
    targets: &[String],
    callers: &[String],
    wasm_rpc_override: WasmRpcOverride,
    stubgen_command: &str,
    stubgen_prefix: &[&str],
) {
    makefile.add_task(Task::Default);
    makefile.add_task(Task::Clean);
    makefile.add_task(Task::Build { release: false });
    makefile.add_task(Task::Build { release: true });
    makefile.add_task(Task::Test);

    let mut regenerate_stub_tasks = Vec::new();
    for target in targets {
        let generate_stub_task = Task::GenerateStub {
            target: target.clone(),
            wasm_rpc_override: wasm_rpc_override.clone(),
            stubgen_command: stubgen_command.to_string(),
            stubgen_prefix: stubgen_prefix.iter().map(|s| s.to_string()).collect(),
        };
        makefile.add_task(generate_stub_task);

        for caller in callers {
            let add_stub_dependency_task = Task::AddStubDependency {
                target: target.clone(),
                caller: caller.clone(),
                stubgen_command: stubgen_command.to_string(),
                stubgen_prefix: stubgen_prefix.iter().map(|s| s.to_string()).collect(),
            };
            regenerate_stub_tasks.push(add_stub_dependency_task.name());
            makefile.add_task(add_stub_dependency_task);
        }
    }

    makefile.add_task(Task::RegenerateStubs {
        dependencies: regenerate_stub_tasks,
    });

    let mut compose_tasks = Vec::new();
    let mut compose_release_tasks = Vec::new();
    for caller in callers {
        let debug_compose_task = Task::Compose {
            release: false,
            caller: caller.clone(),
            targets: targets.to_vec(),
            stubgen_command: stubgen_command.to_string(),
            stubgen_prefix: stubgen_prefix.iter().map(|s| s.to_string()).collect(),
        };

        let release_compose_task = Task::Compose {
            release: true,
            caller: caller.clone(),
            targets: targets.to_vec(),
            stubgen_command: stubgen_command.to_string(),
            stubgen_prefix: stubgen_prefix.iter().map(|s| s.to_string()).collect(),
        };

        compose_tasks.push(debug_compose_task.name());
        compose_release_tasks.push(release_compose_task.name());

        makefile.add_task(debug_compose_task);
        makefile.add_task(release_compose_task);
    }

    makefile.add_task(Task::PostBuild {
        release: false,
        tasks: compose_tasks,
    });
    makefile.add_task(Task::PostBuild {
        release: true,
        tasks: compose_release_tasks,
    });

    makefile.add_task(Task::BuildFlow { release: false });
    makefile.add_task(Task::BuildFlow { release: true });
}

fn has_cargo_make() -> bool {
    Command::new("cargo-make")
        .args(["--version"])
        .output()
        .is_ok()
}
