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

use crate::diagnose::VersionRequirement::{ExactByNameVersion, ExactVersion, MinimumVersion};

use colored::Colorize;
use golem_examples::model::GuestLanguage;
use indoc::indoc;
use regex::Regex;
use std::cmp::max;
use std::collections::HashSet;
use std::fmt::Display;
use std::fmt::Formatter;
use std::path::{Path, PathBuf};
use std::process::Command;
use version_compare::{CompOp, Version};
use walkdir::DirEntry;

pub mod cli {
    use clap::Parser;
    use golem_examples::model::GuestLanguage;

    #[derive(Parser, Debug)]
    #[command()]
    pub struct Command {
        #[arg(short, long, alias = "lang")]
        pub language: Option<GuestLanguage>,
    }
}

#[derive(PartialEq, Eq, Hash, Copy, Clone)]
enum Language {
    CCcp,
    Go,
    JsTs,
    Python,
    Rust,
    Zig,
}

struct SelectedLanguage {
    language: Language,
    project_dir: PathBuf,
    detected_by_reason: Option<String>,
}

impl SelectedLanguage {
    pub fn from_flag(guest_language: GuestLanguage) -> Option<SelectedLanguage> {
        let language = match guest_language {
            GuestLanguage::Rust => Some(Language::Rust),
            GuestLanguage::Go => Some(Language::Go),
            GuestLanguage::C => Some(Language::CCcp),
            GuestLanguage::Zig => Some(Language::Zig),
            GuestLanguage::JavaScript => Some(Language::JsTs),
            GuestLanguage::TypeScript => Some(Language::JsTs),
            GuestLanguage::CSharp => None,
            GuestLanguage::Swift => None,
            GuestLanguage::Grain => None,
            GuestLanguage::Python => Some(Language::Python),
            GuestLanguage::Scala2 => None,
        };

        language.map(|language| SelectedLanguage {
            language,
            project_dir: PathBuf::from("."),
            detected_by_reason: None,
        })
    }

    pub fn from_env() -> Option<SelectedLanguage> {
        let ordered_language_project_hint_files: Vec<(&str, Language)> = vec![
            ("build.zig", Language::Zig),
            ("Cargo.toml", Language::Rust),
            ("go.mod", Language::Go),
            ("package.json", Language::JsTs),
            ("Makefile", Language::CCcp),
            ("main.py", Language::Python),
        ];

        let detect_in_dir = |dir: &Path| -> Option<SelectedLanguage> {
            ordered_language_project_hint_files
                .iter()
                .find_map(|(file, language)| {
                    let path = dir.join(Path::new(file));
                    path.exists().then_some(SelectedLanguage {
                        language: *language,
                        project_dir: path
                            .parent()
                            .expect("Failed to get parent for project file")
                            .to_path_buf(),
                        detected_by_reason: Some(format!(
                            "Detected project file: {}",
                            path.to_string_lossy().green().bold()
                        )),
                    })
                })
        };

        fn is_dir(entry: &DirEntry) -> bool {
            entry
                .metadata()
                .expect("Failed to get dir entry metadata")
                .is_dir()
        }

        fn is_hidden(entry: &DirEntry) -> bool {
            let file_name = entry
                .file_name()
                .to_str()
                .expect("Failed to get file name from dir entry");
            file_name != "." && file_name.starts_with(".")
        }

        // Searching - down
        {
            let language = walkdir::WalkDir::new(".")
                .max_depth(4)
                .into_iter()
                .filter_entry(|e| is_dir(e) && !is_hidden(e))
                .find_map(|e| e.ok().and_then(|e| detect_in_dir(e.path())));

            if language.is_some() {
                return language;
            }
        }

        // Searching - up
        {
            let starting_dir = std::env::current_dir().expect("Failed to get current dir");
            let mut dir = starting_dir.parent();
            loop {
                match dir {
                    Some(d) => match detect_in_dir(d) {
                        result @ Some(_) => return result,
                        None => dir = d.parent(),
                    },
                    None => return None,
                }
            }
        }
    }
}

impl Language {
    pub fn tools(&self) -> Vec<Tool> {
        match self {
            Language::CCcp => vec![Tool::WasiSdk, Tool::WitBindgen, Tool::WasmTools],
            Language::Go => vec![
                Tool::TinyGo,
                Tool::WitBindgen,
                Tool::WasmTools,
                Tool::GolemSdkGo,
            ],
            Language::JsTs => vec![
                Tool::Npm,
                Tool::Jco,
                Tool::ComponentizeJs,
                Tool::GolemSdkTypeScript,
            ],
            Language::Python => vec![Tool::ComponentizePy],
            Language::Rust => vec![
                Tool::RustTargetWasm32Wasi,
                Tool::CargoComponent,
                Tool::GolemSdkRust,
            ],
            Language::Zig => vec![Tool::Zig, Tool::WitBindgen, Tool::WasmTools],
        }
    }

    pub fn language_guide_setup_url(&self) -> Vec<&str> {
        match self {
            Language::CCcp => vec!["https://learn.golem.cloud/docs/ccpp-language-guide/setup"],
            Language::Go => vec!["https://learn.golem.cloud/docs/go-language-guide/setup"],
            Language::JsTs => vec![
                "https://learn.golem.cloud/docs/experimental-languages/js-language-guide/setup",
                "https://learn.golem.cloud/docs/experimental-languages/ts-language-guide/setup",
            ],
            Language::Python => vec!["https://learn.golem.cloud/docs/python-language-guide/setup"],
            Language::Rust => vec!["https://learn.golem.cloud/docs/rust-language-guide/setup"],
            Language::Zig => vec![
                "https://learn.golem.cloud/docs/experimental-languages/zig-language-guide/setup",
            ],
        }
    }

    pub fn tools_with_rpc(&self) -> Vec<Tool> {
        let mut tools = self.tools();
        tools.append(&mut Self::common_rpc_tools());
        tools
    }

    pub fn common_rpc_tools() -> Vec<Tool> {
        vec![Tool::RustTargetWasm32Wasi]
    }
}

impl Display for Language {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::CCcp => f.write_str("C or C++"),
            Language::Go => f.write_str("Go"),
            Language::JsTs => f.write_str("JavaScript or TypeScript"),
            Language::Python => f.write_str("Python"),
            Language::Rust => f.write_str("Rust"),
            Language::Zig => f.write_str("Zig"),
        }
    }
}

#[allow(clippy::enum_variant_names)]
enum VersionRequirement {
    ExactVersion(&'static str),
    ExactByNameVersion(&'static str),
    MinimumVersion(&'static str),
}

impl VersionRequirement {
    pub fn as_str(&self) -> &str {
        match self {
            ExactVersion(version) => version,
            ExactByNameVersion(version) => version,
            MinimumVersion(version) => version,
        }
    }
}

enum VersionRelation {
    OkEqual,
    OkNewer,
    KoNotEqual,
    KoNewer,
    KoOlder,
    Error,
}

impl VersionRelation {
    fn is_ok(&self) -> bool {
        match self {
            VersionRelation::OkEqual => true,
            VersionRelation::OkNewer => true,
            VersionRelation::KoNotEqual => false,
            VersionRelation::KoNewer => false,
            VersionRelation::KoOlder => false,
            VersionRelation::Error => false,
        }
    }
}

impl Display for VersionRelation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionRelation::OkEqual => f.write_str("[equal: ok]".green().to_string().as_str()),
            VersionRelation::OkNewer => f.write_str("[newer: ok]".green().to_string().as_str()),
            VersionRelation::KoNotEqual => f.write_str("[<--->: !!]".red().to_string().as_str()),
            VersionRelation::KoNewer => f.write_str("[newer: !!]".red().to_string().as_str()),
            VersionRelation::KoOlder => f.write_str("[older: !!]".red().to_string().as_str()),
            VersionRelation::Error => f.write_str("[error: !!]".red().to_string().as_str()),
        }
    }
}

struct ToolMetadata {
    pub short_name: &'static str,
    pub description: &'static str,
    pub version_requirement: VersionRequirement,
    pub instructions: &'static str,
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
enum Tool {
    Cargo,
    CargoComponent,
    ComponentizeJs,
    ComponentizePy,
    Go,
    GolemSdkGo,
    GolemSdkRust,
    GolemSdkTypeScript,
    Jco,
    Node,
    Npm,
    Pip,
    Python,
    Rustc,
    Rustup,
    RustTargetWasm32Wasi,
    TinyGo,
    WasiSdk,
    WasmTools,
    WitBindgen,
    Zig,
}

impl Tool {
    pub fn metadata(&self) -> ToolMetadata {
        match self {
            Tool::Cargo => ToolMetadata {
                short_name: "cargo",
                description: "Rust package manager",
                version_requirement: MinimumVersion("1.80.1"),
                instructions:
                    "See the rustup step above (https://www.rust-lang.org/learn/get-started)",
            },
            Tool::CargoComponent => ToolMetadata {
                short_name: "cargo-component",
                description: "Cargo subcommand for building WebAssembly components",
                version_requirement: ExactVersion("0.13.2"),
                instructions: indoc! {"
                    Install the following specific version of cargo-component:
                        cargo install --force --locked cargo-component@0.13.2

                    For more information see:
                        https://github.com/bytecodealliance/cargo-component

                    Check PATH for $HOME/.cargo/bin
                "},
            },
            Tool::ComponentizeJs => ToolMetadata {
                short_name: "componentize-js",
                description:
                    "Tool for converting JavaScript applications to WebAssembly components",
                version_requirement: MinimumVersion("0.10.5-golem.3"),
                instructions: indoc! {"
                    Add latest componentize-js as dependency:
                        npm install --save-dev @golemcloud/componentize-js

                    For more information see:
                        JavaScript: https://learn.golem.cloud/docs/experimental-languages/js-language-guide/golem-js-sdk
                        TypeScript: https://learn.golem.cloud/docs/experimental-languages/ts-language-guide/golem-ts-sdk
                "},
            },
            Tool::ComponentizePy => ToolMetadata {
                short_name: "componentize-py",
                description: "Tool for converting Python applications to WebAssembly components",
                version_requirement: ExactVersion("0.13.5"),
                instructions: indoc! {"
                    Install the following specific version:
                        pip install componentize-py==0.13.5

                    For more information see:
                        https://github.com/bytecodealliance/componentize-py
                "},
            },
            Tool::Go => ToolMetadata {
                short_name: "go",
                description: "Go language tooling",
                version_requirement: MinimumVersion("1.20.0"),
                instructions: indoc! {"
                    Install the latest stable go tooling: https://go.dev/doc/install
                "},
            },
            Tool::GolemSdkGo => ToolMetadata {
                short_name: "golem-go",
                description: "Golem SDK for Go",
                version_requirement: MinimumVersion("0.7.0"),
                instructions: indoc! {"
                    Add latest golem-go as dependency:
                        go get github.com/golemcloud/golem-go

                    For more information see:
                        https://learn.golem.cloud/docs/go-language-guide/golem-go-sdk
                "},
            },
            Tool::GolemSdkRust => ToolMetadata {
                short_name: "golem-rust",
                description: "Golem SDK for Rust",
                version_requirement: MinimumVersion("1.0.0"),
                instructions: indoc! {"
                    Add latest golem-rust as dependency:
                        cargo add golem-rust
                "},
            },
            Tool::GolemSdkTypeScript => ToolMetadata {
                short_name: "golem-ts",
                description: "Golem SDK for JavaScript and TypeScript",
                version_requirement: MinimumVersion("0.2.0"),
                instructions: indoc! {"
                    Add latest golem-ts as dependency:
                        npm install --save-dev @golemcloud/golem-ts

                    For more information see:
                        JavaScript: https://learn.golem.cloud/docs/experimental-languages/js-language-guide/golem-js-sdk
                        TypeScript: https://learn.golem.cloud/docs/experimental-languages/ts-language-guide/golem-ts-sdk
                "},
            },
            Tool::Jco => ToolMetadata {
                short_name: "jco",
                description: "Toolchain for working with WebAssembly Components in JavaScript",
                version_requirement: MinimumVersion("1.4.4-golem.1"),
                instructions: indoc! {"
                    Add latest jco as dependency:
                        npm install --save-dev @golemcloud/jco

                    For more information see:
                        JavaScript: https://learn.golem.cloud/docs/experimental-languages/js-language-guide/golem-js-sdk
                        TypeScript: https://learn.golem.cloud/docs/experimental-languages/ts-language-guide/golem-ts-sdk
                "},
            },
            Tool::Node => ToolMetadata {
                short_name: "node",
                description: "JavaScript runtime",
                version_requirement: MinimumVersion("20.17.0"),
                instructions: indoc! {"
                    Install latest stable node and npm:
                        https://docs.npmjs.com/downloading-and-installing-node-js-and-npm
                "},
            },
            Tool::Npm => ToolMetadata {
                short_name: "npm",
                description: "Node package manager",
                version_requirement: MinimumVersion("10.8.2"),
                instructions: indoc! {"
                    See node above (https://docs.npmjs.com/downloading-and-installing-node-js-and-npm)
                "},
            },
            Tool::Pip => ToolMetadata {
                short_name: "pip",
                description: "Python package installer",
                version_requirement: MinimumVersion("24.0"),
                instructions: indoc! {"
                    Install latest pip: https://pip.pypa.io/en/stable/installation/
                "},
            },
            Tool::Python => ToolMetadata {
                short_name: "python",
                description: "Python interpreter",
                version_requirement: MinimumVersion("3.10"),
                instructions: indoc! {"
                    Install python: https://www.python.org/
                "},
            },
            Tool::Rustc => ToolMetadata {
                short_name: "rustc",
                description: "Rust compiler",
                version_requirement: MinimumVersion("1.80.1"),
                instructions: indoc! {"
                    See the rustup step above (https://www.rust-lang.org/learn/get-started),
                    then install latest stable rust:
                        rustup install stable && rustup default stable
                "},
            },
            Tool::Rustup => ToolMetadata {
                short_name: "rustup",
                description: "Rust toolchain installer",
                version_requirement: MinimumVersion("1.27.1"),
                instructions: indoc! {"
                    Install rust tooling with rustup:
                        https://www.rust-lang.org/learn/get-started

                    For macos and linux:
                        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

                    Check PATH for $HOME/.cargo/bin
                "},
            },
            Tool::RustTargetWasm32Wasi => ToolMetadata {
                short_name: "rust target wasm32-wasi",
                description: "Rust target for building WebAssembly components",
                version_requirement: ExactByNameVersion("wasm32-wasi"),
                instructions: indoc! {"
                    Install WebAssembly target for rust:
                        rustup target add wasm32-wasi
                "},
            },
            Tool::TinyGo => ToolMetadata {
                short_name: "tinygo",
                description: "Go compiler for WebAssembly (and embedded systems)",
                version_requirement: MinimumVersion("0.33"),
                instructions: indoc! {"
                    Install latest TinyGo:
                        https://tinygo.org/getting-started/install/

                    For macos use:
                        brew tap tinygo-org/tools
                        brew install tinygo
                "},
            },
            Tool::WasiSdk => ToolMetadata {
                short_name: "wasi-sdk",
                description: "WebAssembly toolchain for C and C++",
                // NOTE: Version is not detectable currently, from 24.0 it will be stored in a version file
                version_requirement: ExactByNameVersion("WASI_SDK set"),
                instructions: indoc! {"
                    Install WASI SDK 23.0:
                        https://github.com/WebAssembly/wasi-sdk

                    Don't forget to export the WASI_SDK environment variable!
                "},
            },
            Tool::WasmTools => ToolMetadata {
                short_name: "wasm-tools",
                description: "Tools for manipulation of WebAssembly modules",
                version_requirement: ExactVersion("1.210.0"),
                instructions: indoc! {"
                    Install the following specific version of wasm-tools:
                        cargo install --force --locked  wasm-tools@1.210.0
                "},
            },
            Tool::WitBindgen => ToolMetadata {
                short_name: "wit-bindgen",
                description: "Guest language bindings generator for WIT",
                version_requirement: ExactVersion("0.26.0"),
                instructions: indoc! {"
                    Install the following specific version of wit-bindgen:
                        cargo install --force --locked wit-bindgen-cli@0.26.0
                "},
            },
            Tool::Zig => ToolMetadata {
                short_name: "zig",
                description: "Zig language tooling",
                version_requirement: MinimumVersion("0.13.0"),
                instructions: indoc! {"
                    Install latest version of Zig:
                        https://ziglang.org/learn/getting-started/#installing-zig
                "},
            },
        }
    }

    pub fn direct_dependencies(&self) -> Vec<Tool> {
        match self {
            Tool::Cargo => vec![Tool::Rustup],
            Tool::CargoComponent => vec![Tool::Cargo],
            Tool::ComponentizeJs => vec![Tool::Npm],
            Tool::ComponentizePy => vec![Tool::Pip],
            Tool::Go => vec![],
            Tool::GolemSdkGo => vec![Tool::Go],
            Tool::GolemSdkRust => vec![Tool::Cargo],
            Tool::GolemSdkTypeScript => vec![Tool::Npm],
            Tool::Jco => vec![Tool::Npm],
            Tool::Node => vec![],
            Tool::Npm => vec![Tool::Node],
            Tool::Pip => vec![Tool::Python],
            Tool::Python => vec![],
            Tool::Rustc => vec![Tool::Rustup],
            Tool::Rustup => vec![],
            Tool::RustTargetWasm32Wasi => vec![Tool::Rustc],
            Tool::TinyGo => vec![Tool::Go],
            Tool::WasiSdk => vec![],
            Tool::WasmTools => vec![Tool::Cargo],
            Tool::WitBindgen => vec![Tool::Cargo],
            Tool::Zig => vec![],
        }
    }

    pub fn with_all_dependencies(tools: Vec<Tool>) -> Vec<Tool> {
        let mut visited_tools = HashSet::<Tool>::new();
        let mut all_tools = Vec::<Tool>::new();

        Self::collect_deps(&tools, &mut visited_tools, &mut all_tools);

        all_tools
    }

    fn collect_deps(
        tools: &Vec<Tool>,
        visited_tools: &mut HashSet<Tool>,
        collected_deps: &mut Vec<Tool>,
    ) {
        for tool in tools {
            if visited_tools.contains(tool) {
                continue;
            }
            visited_tools.insert(*tool);
            Self::collect_deps(&tool.direct_dependencies(), visited_tools, collected_deps);
            collected_deps.push(*tool);
        }
    }

    pub fn get_version(&self, dir: &Path) -> Result<String, String> {
        let version_regex = Regex::new(r"[^0-9]*([0-9]+\.[0-9]+(\.[0-9]+)?).*")
            .expect("Failed to compile common version regex");

        match self {
            Tool::Cargo => cmd_version(dir, "cargo", vec!["version"], &version_regex),
            Tool::CargoComponent => {
                cmd_version(dir, "cargo-component", vec!["--version"], &version_regex)
            }
            Tool::ComponentizeJs => npm_package_version(dir, "@golemcloud/componentize-js"),
            Tool::ComponentizePy => {
                cmd_version(dir, "componentize-py", vec!["--version"], &version_regex)
            }
            Tool::Go => cmd_version(dir, "go", vec!["version"], &version_regex),
            Tool::GolemSdkGo => go_mod_version(dir, "github.com/golemcloud/golem-go"),
            Tool::GolemSdkRust => rust_package_version(dir, "golem-rust"),
            Tool::GolemSdkTypeScript => npm_package_version(dir, "@golemcloud/golem-ts"),
            Tool::Jco => npm_package_version(dir, "@golemcloud/jco"),
            Tool::Node => cmd_version(dir, "node", vec!["--version"], &version_regex),
            Tool::Npm => cmd_version(dir, "npm", vec!["--version"], &version_regex),
            Tool::Pip => cmd_version(dir, "pip", vec!["--version"], &version_regex),
            Tool::Python => cmd_version(dir, "python", vec!["--version"], &version_regex),
            Tool::Rustc => cmd_version(dir, "rustc", vec!["--version"], &version_regex),
            Tool::Rustup => cmd_version(dir, "rustup", vec!["--version"], &version_regex),
            Tool::RustTargetWasm32Wasi => rust_target(dir, "wasm32-wasi"),
            Tool::TinyGo => cmd_version(dir, "tinygo", vec!["version"], &version_regex),
            Tool::WasiSdk => std::env::var("WASI_SDK")
                .map(|_| "WASI_SDK set".to_string())
                .map_err(|_| "WASI_SDK no set".to_string()),
            Tool::WasmTools => cmd_version(dir, "wasm-tools", vec!["--version"], &version_regex),
            Tool::WitBindgen => cmd_version(dir, "wit-bindgen", vec!["--version"], &version_regex),
            Tool::Zig => cmd_version(dir, "zig", vec!["version"], &version_regex),
        }
    }
}

struct DetectedTool {
    metadata: ToolMetadata,
    version: Option<String>,
    version_relation: VersionRelation,
    details: String,
}

impl DetectedTool {
    pub fn new(dir: &Path, tool: Tool) -> Self {
        let metadata = tool.metadata();
        let version = tool.get_version(dir);
        let required_version = &metadata.version_requirement;

        fn compare(
            required_version: &str,
            version: &str,
            newer_accepted: bool,
        ) -> Result<VersionRelation, String> {
            match Version::from(required_version) {
                Some(required_version) => match Version::from(version) {
                    Some(version) => {
                        let comp_op = version.compare(&required_version);
                        match comp_op {
                            CompOp::Eq => Ok(VersionRelation::OkEqual),
                            CompOp::Lt => Ok(VersionRelation::KoOlder),
                            CompOp::Gt if newer_accepted => Ok(VersionRelation::OkNewer),
                            CompOp::Gt => Ok(VersionRelation::KoNewer),
                            _ => Err(format!("Unexpected compare result: {}", comp_op.sign())),
                        }
                    }
                    None => Err(format!("Failed to parse detected version: {}", version)),
                },
                None => Err(format!(
                    "Failed to parse required version: {}",
                    required_version
                )),
            }
        }

        let result = match &version {
            Ok(version) => match &required_version {
                ExactVersion(required_version) => compare(required_version, version, false),
                ExactByNameVersion(required_version) => {
                    if version == required_version {
                        Ok(VersionRelation::OkEqual)
                    } else {
                        Ok(VersionRelation::KoNotEqual)
                    }
                }
                MinimumVersion(required_version) => compare(required_version, version, true),
            },
            Err(error) => Err(format!("Failed to get tool version: {}", error)),
        };

        match result {
            Ok(relation) => {
                let sign = match relation {
                    VersionRelation::OkEqual => "==",
                    VersionRelation::OkNewer => ">",
                    VersionRelation::KoNotEqual => "!=",
                    VersionRelation::KoNewer => ">",
                    VersionRelation::KoOlder => "<",
                    VersionRelation::Error => "!!",
                };

                Self {
                    metadata: tool.metadata(),
                    version: version.ok(),
                    version_relation: relation,
                    details: format!("({}{})", sign, required_version.as_str()),
                }
            }
            Err(details) => Self {
                metadata: tool.metadata(),
                version: None,
                version_relation: VersionRelation::Error,
                details,
            },
        }
    }
}

pub fn diagnose(command: cli::Command) {
    let selected_language = match &command.language {
        Some(language) => SelectedLanguage::from_flag(language.clone()),
        None => SelectedLanguage::from_env(),
    };

    match &selected_language {
        Some(selected_language) => {
            match &selected_language.detected_by_reason {
                Some(reason) => {
                    println!("{}", reason);
                    println!(
                        "Detected language: {} (to explicitly specify the language use the --language flag)",
                        selected_language.language.to_string().bold().green(),
                    );
                }
                None => {
                    println!(
                        "Explicitly selected language: {}",
                        selected_language.language
                    )
                }
            }
            println!("Online language setup guide(s):");
            for url in selected_language.language.language_guide_setup_url() {
                println!("  {}", url.bold().underline());
            }
            println!();

            report_tools(
                Tool::with_all_dependencies(selected_language.language.tools_with_rpc())
                    .iter()
                    .map(|t| DetectedTool::new(&selected_language.project_dir, *t))
                    .collect(),
            );
        }
        None => match &command.language {
            Some(language) => {
                println!(
                    "The selected language ({}) has no language specific diagnostics currently.\n",
                    language
                );
                println!("Running diagnostics for common Worker to Worker RPC tooling.\n");

                let dir = PathBuf::from(".");
                report_tools(
                    Tool::with_all_dependencies(Language::common_rpc_tools())
                        .iter()
                        .map(|t| DetectedTool::new(&dir, *t))
                        .collect(),
                );
            }
            None => {
                println!("No language detected, use the --language flag to explicitly specify one");
            }
        },
    }
}

fn report_tools(all_tools: Vec<DetectedTool>) {
    let (name_padding, version_padding) = {
        let mut name_padding = 0;
        let mut version_padding = 0;
        for tool in &all_tools {
            name_padding = max(name_padding, tool.metadata.short_name.len());
            version_padding = max(
                version_padding,
                tool.version.as_ref().map(|v| v.len()).unwrap_or(0),
            );
        }
        (name_padding + 1, version_padding + 1)
    };

    println!("Recommended tooling:");
    for tool in &all_tools {
        println!(
            "  {: <width$} {}",
            format!("{}:", tool.metadata.short_name),
            tool.metadata.description,
            width = name_padding,
        );
    }
    println!();

    println!("Installed tool versions:");
    for tool in &all_tools {
        println!(
            "  {: <name_padding$} {} {: <version_padding$}{}",
            format!("{}:", tool.metadata.short_name),
            tool.version_relation,
            tool.version.clone().unwrap_or_else(|| "".to_string()),
            tool.details,
        );
    }
    println!();

    let non_ok_tools: Vec<_> = all_tools
        .into_iter()
        .filter(|t| !t.version_relation.is_ok())
        .collect();

    if non_ok_tools.is_empty() {
        println!("{}", "All tools are ok.".green())
    } else {
        println!("{}", "Recommended steps:".yellow());
        for tool in &non_ok_tools {
            println!();
            println!(
                "  {}",
                format!(
                    "{}: {}",
                    tool.metadata.short_name, tool.metadata.description
                )
                .underline()
            );
            println!(
                "    Problem: {} {}",
                tool.version_relation,
                tool.version
                    .clone()
                    .unwrap_or_else(|| tool.details.to_string()),
            );
            println!();
            println!("    Instructions:");
            for line in tool.metadata.instructions.lines() {
                println!("      {}", line.yellow());
            }
        }
    }
}

fn cmd_version(dir: &Path, cmd: &str, args: Vec<&str>, regex: &Regex) -> Result<String, String> {
    match Command::new(cmd).current_dir(dir).args(args).output() {
        Ok(result) => {
            let output = String::from_utf8_lossy(&result.stdout);
            match regex.captures(&output).and_then(|c| c.get(1)) {
                Some(version) => Ok(version.as_str().to_string()),
                None => Err(format!("Failed to extract version from output: {}", output)),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

fn rust_target(dir: &Path, target_name: &str) -> Result<String, String> {
    dep_version(
        dir,
        "rustup",
        vec!["target", "list", "--installed"],
        |dep| (dep == target_name).then(|| target_name.to_string()),
    )
}

fn rust_package_version(dir: &Path, package_name: &str) -> Result<String, String> {
    dep_version(dir, "cargo", vec!["tree", "--prefix", "none"], |dep| {
        let tokens: Vec<_> = dep.split(' ').collect();
        (tokens.len() >= 2 && tokens[0] == package_name && tokens[1].starts_with("v"))
            .then(|| tokens[1][1..].to_string())
    })
}

fn npm_package_version(dir: &Path, package_name: &str) -> Result<String, String> {
    fn trim_scope_symbol(s: &str) -> &str {
        s.starts_with("@").then(|| &s[1..]).unwrap_or(s)
    }

    let package_name = trim_scope_symbol(package_name);
    dep_version(dir, "npm", vec!["list", "--parseable", "--long"], |dep| {
        let tokens: Vec<_> = dep.split(':').collect();
        (tokens.len() == 2)
            .then(|| trim_scope_symbol(tokens[1]).split("@").collect::<Vec<_>>())
            .and_then(|tokens| {
                (tokens.len() == 2 && tokens[0] == package_name).then(|| tokens[1].to_string())
            })
    })
}

fn go_mod_version(dir: &Path, module_name: &str) -> Result<String, String> {
    dep_version(dir, "go", vec!["list", "-m", "all"], |dep| {
        let tokens: Vec<_> = dep.split(' ').collect();
        (tokens.len() == 2 && tokens[0] == module_name && tokens[1].starts_with("v"))
            .then(|| tokens[1][1..].to_string())
    })
}

fn dep_version<F>(dir: &Path, cmd: &str, args: Vec<&str>, find: F) -> Result<String, String>
where
    F: FnMut(&str) -> Option<String>,
{
    match Command::new(cmd).current_dir(dir).args(args).output() {
        Ok(result) => {
            let output = String::from_utf8_lossy(&result.stdout);
            output
                .lines()
                .find_map(find)
                .ok_or_else(|| "Dependency not found".to_string())
        }
        Err(err) => Err(err.to_string()),
    }
}
