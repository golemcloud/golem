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

use crate::diagnose::VersionRequirement::{ExactByNameVersion, ExactVersion, MinimumVersion};
use crate::log::logln;
use anyhow::{anyhow, Context};
use colored::Colorize;
use golem_templates::model::GuestLanguage;
use indoc::indoc;
use regex::Regex;
use std::cmp::max;
use std::collections::HashSet;
use std::fmt::Display;
use std::fmt::Formatter;
use std::path::{Path, PathBuf};
use std::process::Command;
use version_compare::{Cmp, Version};
use walkdir::DirEntry;
use wax::{Glob, LinkBehavior, WalkBehavior};

#[derive(PartialEq, Eq, Hash, Copy, Clone)]
enum Language {
    CCcp,
    Go,
    JsTs,
    Python,
    Rust,
    Zig,
    ScalaJs,
    MoonBit,
}

struct SelectedLanguage {
    language: Language,
    project_dir: PathBuf,
    detected_by_reason: Option<String>,
}

impl SelectedLanguage {
    pub fn from_flag(dir: &Path, guest_language: GuestLanguage) -> Option<SelectedLanguage> {
        let language = match guest_language {
            GuestLanguage::Rust => Some(Language::Rust),
            GuestLanguage::TypeScript => Some(Language::JsTs),
        };

        language.map(|language| SelectedLanguage {
            language,
            project_dir: dir.to_path_buf(),
            detected_by_reason: None,
        })
    }

    pub fn from_env(dir: &Path) -> Option<SelectedLanguage> {
        let ordered_language_project_hint_patterns: Vec<(&str, Language)> = vec![
            ("build.zig", Language::Zig),
            ("*.scala", Language::ScalaJs),
            ("build.sbt", Language::ScalaJs),
            ("Cargo.toml", Language::Rust),
            ("go.mod", Language::Go),
            ("package.json", Language::JsTs),
            ("Makefile", Language::CCcp),
            ("main.py", Language::Python),
            ("*.c", Language::CCcp),
            ("*.h", Language::CCcp),
            ("*.cpp", Language::CCcp),
            ("*.go", Language::Go),
            ("*.js", Language::JsTs),
            ("*.ts", Language::JsTs),
            ("*.py", Language::Python),
            ("*.rs", Language::Rust),
            ("*.zig", Language::Zig),
            ("*.mbt", Language::MoonBit),
        ];

        let detect_in_dir = |dir: &Path| -> Option<SelectedLanguage> {
            ordered_language_project_hint_patterns
                .iter()
                .find_map(|(file_pattern, language)| {
                    let glob = Glob::new(file_pattern)
                        .with_context(|| {
                            anyhow!("Failed to compile file hint pattern: {}", file_pattern)
                        })
                        .unwrap();

                    let file_match = glob
                        .walk_with_behavior(
                            dir,
                            WalkBehavior {
                                link: LinkBehavior::ReadFile,
                                ..WalkBehavior::default()
                            },
                        )
                        .next()
                        .and_then(|item| item.ok())
                        .map(|item| item.path().to_path_buf());

                    file_match.map(|file_match| SelectedLanguage {
                        language: *language,
                        project_dir: dir.to_path_buf(),
                        detected_by_reason: Some(format!(
                            "Detected project file: {}",
                            file_match.display().to_string().green().bold()
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
            let language = walkdir::WalkDir::new(dir)
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
            Language::CCcp => vec![
                Tool::WasiSdk,
                Tool::WitBindgen,
                Tool::WasmTools,
                Tool::CMake,
            ],
            Language::Go => vec![Tool::Go, Tool::TinyGo, Tool::WasmTools, Tool::GolemSdkGo],
            Language::JsTs => vec![
                Tool::Npm,
                Tool::Jco,
                Tool::ComponentizeJs,
                Tool::GolemSdkTypeScript,
            ],
            Language::Python => vec![Tool::Uv, Tool::ComponentizePy],
            Language::Rust => vec![
                Tool::RustTargetWasm32WasiP1,
                Tool::CargoComponent,
                Tool::GolemSdkRust,
            ],
            Language::Zig => vec![Tool::Zig, Tool::WitBindgen, Tool::WasmTools],
            Language::ScalaJs => vec![Tool::Npm, Tool::Sbt, Tool::WitBindgenScalaJs],
            Language::MoonBit => vec![Tool::WasmTools, Tool::WitBindgen, Tool::MoonBit],
        }
    }

    pub fn language_guide_setup_url(&self) -> Vec<&str> {
        match self {
            Language::CCcp => vec!["https://learn.golem.cloud/ccpp-language-guide/setup"],
            Language::Go => vec!["https://learn.golem.cloud/go-language-guide/setup"],
            Language::JsTs => vec![
                "https://learn.golem.cloud/js-language-guide/setup",
                "https://learn.golem.cloud/ts-language-guide/setup",
            ],
            Language::Python => vec!["https://learn.golem.cloud/python-language-guide/setup"],
            Language::Rust => vec!["https://learn.golem.cloud/rust-language-guide/setup"],
            Language::Zig => {
                vec!["https://learn.golem.cloud/experimental-languages/zig-language-guide/setup"]
            }
            Language::ScalaJs => vec![
                "https://learn.golem.cloud/experimental-languages/scalajs-language-guide/setup",
            ],
            Language::MoonBit => vec![
                "https://learn.golem.cloud/experimental-languages/moonbit-language-guide/setup",
            ],
        }
    }

    pub fn tools_with_rpc(&self) -> Vec<Tool> {
        let mut tools = self.tools();
        tools.append(&mut Self::common_rpc_tools());
        tools
    }

    pub fn common_rpc_tools() -> Vec<Tool> {
        vec![Tool::RustTargetWasm32WasiP1]
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
            Language::ScalaJs => f.write_str("Scala.js"),
            Language::MoonBit => f.write_str("MoonBit"),
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
    CMake,
    Cargo,
    CargoComponent,
    ComponentizeJs,
    ComponentizePy,
    Go,
    GolemSdkGo,
    GolemSdkRust,
    GolemSdkTypeScript,
    Jco,
    Jdk,
    MoonBit,
    Node,
    Npm,
    RustTargetWasm32WasiP1,
    Rustc,
    Rustup,
    Sbt,
    TinyGo,
    Uv,
    WasiSdk,
    WasmTools,
    WitBindgen,
    WitBindgenScalaJs,
    Zig,
}

impl Tool {
    pub fn metadata(&self) -> ToolMetadata {
        match self {
            Tool::Cargo => ToolMetadata {
                short_name: "cargo",
                description: "Rust package manager",
                version_requirement: MinimumVersion("1.84.0"),
                instructions:
                    "See the rustup step above (https://www.rust-lang.org/learn/get-started)",
            },
            Tool::CargoComponent => ToolMetadata {
                short_name: "cargo-component",
                description: "Cargo subcommand for building WebAssembly components",
                version_requirement: ExactVersion("0.20.0"),
                instructions: indoc! {"
                    Install the following specific version of cargo-component:
                        cargo install --force --locked cargo-component@0.20.0

                    For more information see:
                        https://github.com/bytecodealliance/cargo-component

                    Check PATH for $HOME/.cargo/bin
                "},
            },
            Tool::ComponentizeJs => ToolMetadata {
                short_name: "componentize-js",
                description:
                    "Tool for converting JavaScript applications to WebAssembly components",
                version_requirement: MinimumVersion("0.18.0"),
                instructions: indoc! {"
                    Add latest componentize-js as dependency:
                        npm install --save-dev @bytecodealliance/componentize-js

                    For more information see:
                        JavaScript: https://learn.golem.cloud/docs/experimental-languages/js-language-guide/golem-js-sdk
                        TypeScript: https://learn.golem.cloud/docs/experimental-languages/ts-language-guide/golem-ts-sdk
                "},
            },
            Tool::ComponentizePy => ToolMetadata {
                short_name: "componentize-py",
                description: "Tool for converting Python applications to WebAssembly components",
                version_requirement: ExactVersion("0.16.0"),
                instructions: indoc! {"
                    Install the following specific version:
                        uv pip install componentize-py==0.16.0

                    For more information see:
                        https://github.com/bytecodealliance/componentize-py
                "},
            },
            Tool::Go => ToolMetadata {
                short_name: "go",
                description: "Go language tooling",
                version_requirement: MinimumVersion("1.24.0"),
                instructions: indoc! {"
                    Install the latest stable go tooling: https://go.dev/doc/install
                "},
            },
            Tool::GolemSdkGo => ToolMetadata {
                short_name: "golem-go",
                description: "Golem SDK for Go",
                version_requirement: MinimumVersion("1.3.1"),
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
                version_requirement: MinimumVersion("1.5.1"),
                instructions: indoc! {"
                    Add latest golem-rust as dependency:
                        cargo add golem-rust
                "},
            },
            Tool::GolemSdkTypeScript => ToolMetadata {
                short_name: "golem-ts",
                description: "Golem SDK for JavaScript and TypeScript",
                version_requirement: MinimumVersion("1.3.1"),
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
                version_requirement: MinimumVersion("1.10.2"),
                instructions: indoc! {"
                    Add latest jco as dependency:
                        npm install --save-dev @bytecodealliance/jco

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
            Tool::Rustc => ToolMetadata {
                short_name: "rustc",
                description: "Rust compiler",
                version_requirement: MinimumVersion("1.84.0"),
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
            Tool::RustTargetWasm32WasiP1 => ToolMetadata {
                short_name: "rust target wasm32-wasip1",
                description: "Rust target for building WebAssembly components",
                version_requirement: ExactByNameVersion("wasm32-wasip1"),
                instructions: indoc! {"
                    Install WebAssembly target for rust:
                        rustup target add wasm32-wasip1
                "},
            },
            Tool::TinyGo => ToolMetadata {
                short_name: "tinygo",
                description: "Go compiler for WebAssembly (and embedded systems)",
                version_requirement: MinimumVersion("0.37"),
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
                version_requirement: MinimumVersion("25.0"),
                instructions: indoc! {"
                    Install WASI SDK 23.0:
                        https://github.com/WebAssembly/wasi-sdk

                    Don't forget to export the WASI_SDK environment variable!
                "},
            },
            Tool::WasmTools => ToolMetadata {
                short_name: "wasm-tools",
                description: "Tools for manipulation of WebAssembly modules",
                version_requirement: MinimumVersion("1.223.0"),
                instructions: indoc! {"
                    Install the following specific version of wasm-tools:
                        cargo install --force --locked  wasm-tools@1.223.0
                "},
            },
            Tool::WitBindgen => ToolMetadata {
                short_name: "wit-bindgen",
                description: "Guest language bindings generator for WIT",
                version_requirement: ExactVersion("0.40.0"),
                instructions: indoc! {"
                    Install the following specific version of wit-bindgen:
                        cargo install --force --locked wit-bindgen-cli@0.40.0
                "},
            },
            Tool::Zig => ToolMetadata {
                short_name: "zig",
                description: "Zig language tooling",
                version_requirement: MinimumVersion("0.14.0"),
                instructions: indoc! {"
                    Install latest version of Zig:
                        https://ziglang.org/learn/getting-started/#installing-zig
                "},
            },
            Tool::CMake => ToolMetadata {
                short_name: "cmake",
                description: "CMake build system",
                version_requirement: MinimumVersion("3.27.8"),
                instructions: indoc! {"
                    Install latest version of CMake:
                        https://cmake.org/download/

                    For macos use:
                        brew install cmake
                "},
            },
            Tool::Jdk => ToolMetadata {
                short_name: "jdk",
                description: "Java Development Kit",
                version_requirement: MinimumVersion("17.0.0"),
                instructions: indoc! {"
                    Install the latest Java Development Kit:
                        https://www.oracle.com/java/technologies/downloads/
                "},
            },
            Tool::MoonBit => ToolMetadata {
                short_name: "moonbit",
                description: "MoonBit language",
                version_requirement: MinimumVersion("0.1.20250310"),
                instructions: indoc! {"
                    Install latest version of MoonBit:
                        https://www.moonbitlang.com/download/
                "},
            },
            Tool::Sbt => ToolMetadata {
                short_name: "sbt",
                description: "Scala Build Tool",
                version_requirement: MinimumVersion("1.10.7"),
                instructions: indoc! {"
                    Install latest version of SBT:
                        https://www.scala-sbt.org/download/

                    For macos use:
                        brew install sbt
                "},
            },
            Tool::Uv => ToolMetadata {
                short_name: "uv",
                description: "uv - a python package manager",
                version_requirement: MinimumVersion("0.7.0"),
                instructions: indoc! {"
                    Install latest version of UV:
                        https://github.com/astral-sh/uv

                    For macos use:
                        brew install uv
                "},
            },
            Tool::WitBindgenScalaJs => ToolMetadata {
                short_name: "wit-bindgen-scalajs",
                description: "WIT binding generator for Scala.js",
                version_requirement: ExactVersion("0.37.0"),
                instructions: indoc! {"
                        Install the latest version of wit-bindgen-scalajs:
                            cargo install --git https://github.com/vigoo/wit-bindgen-scalajs wit-bindgen-cli --locked
                    "},
            },
        }
    }

    pub fn direct_dependencies(&self) -> Vec<Tool> {
        match self {
            Tool::CMake => vec![],
            Tool::Cargo => vec![Tool::Rustup],
            Tool::CargoComponent => vec![Tool::Cargo],
            Tool::ComponentizeJs => vec![Tool::Npm],
            Tool::ComponentizePy => vec![Tool::Uv],
            Tool::Go => vec![],
            Tool::GolemSdkGo => vec![Tool::Go],
            Tool::GolemSdkRust => vec![Tool::Cargo],
            Tool::GolemSdkTypeScript => vec![Tool::Npm],
            Tool::Jco => vec![Tool::Npm],
            Tool::Jdk => vec![],
            Tool::MoonBit => vec![],
            Tool::Node => vec![],
            Tool::Npm => vec![Tool::Node],
            Tool::RustTargetWasm32WasiP1 => vec![Tool::Rustc],
            Tool::Rustc => vec![Tool::Rustup],
            Tool::Rustup => vec![],
            Tool::Sbt => vec![Tool::Jdk],
            Tool::TinyGo => vec![Tool::Go],
            Tool::Uv => vec![],
            Tool::WasiSdk => vec![],
            Tool::WasmTools => vec![Tool::Cargo],
            Tool::WitBindgen => vec![Tool::Cargo],
            Tool::WitBindgenScalaJs => vec![Tool::Cargo],
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
            Tool::ComponentizeJs => {
                npm_package_version(&find_node_modules(dir), "@bytecodealliance/componentize-js")
            }
            Tool::ComponentizePy => cmd_version(
                dir,
                "uv",
                vec!["run", "componentize-py", "--version"],
                &version_regex,
            ),
            Tool::Go => cmd_version(dir, "go", vec!["version"], &version_regex),
            Tool::GolemSdkGo => go_mod_version(dir, "github.com/golemcloud/golem-go"),
            Tool::GolemSdkRust => rust_package_version(dir, "golem-rust"),
            Tool::GolemSdkTypeScript => {
                npm_package_version(&find_node_modules(dir), "@golemcloud/golem-ts")
            }
            Tool::Jco => npm_package_version(&find_node_modules(dir), "@bytecodealliance/jco"),
            Tool::Node => cmd_version(dir, "node", vec!["--version"], &version_regex),
            Tool::Npm => cmd_version(dir, "npm", vec!["--version"], &version_regex),
            Tool::Rustc => cmd_version(dir, "rustc", vec!["--version"], &version_regex),
            Tool::Rustup => cmd_version(dir, "rustup", vec!["--version"], &version_regex),
            Tool::RustTargetWasm32WasiP1 => rust_target(dir, "wasm32-wasip1"),
            Tool::TinyGo => cmd_version(dir, "tinygo", vec!["version"], &version_regex),
            Tool::WasiSdk => {
                let wasi_sdk_path = std::env::var("WASI_SDK_PATH")
                    .map_err(|_| "WASI_SDK_PATH not set".to_string())?;
                let wasi_sdk_version_file = Path::new(&wasi_sdk_path).join("VERSION");
                let versions = std::fs::read_to_string(&wasi_sdk_version_file).map_err(|err| {
                    format!(
                        "Failed to open {}: {}",
                        wasi_sdk_version_file.to_string_lossy(),
                        err
                    )
                })?;
                versions
                    .lines()
                    .next()
                    .ok_or_else(|| {
                        format!(
                            "Version not found in {}",
                            wasi_sdk_version_file.to_string_lossy()
                        )
                    })
                    .map(|version| version.to_string())
            }
            Tool::WasmTools => cmd_version(dir, "wasm-tools", vec!["--version"], &version_regex),
            Tool::WitBindgen => cmd_version(dir, "wit-bindgen", vec!["--version"], &version_regex),
            Tool::Zig => cmd_version(dir, "zig", vec!["version"], &version_regex),
            Tool::CMake => cmd_version(dir, "cmake", vec!["--version"], &version_regex),
            Tool::Jdk => cmd_version(dir, "javac", vec!["-version"], &version_regex),
            Tool::MoonBit => cmd_version(dir, "moon", vec!["version"], &version_regex),
            Tool::Sbt => cmd_version(dir, "sbt", vec!["--version"], &version_regex),
            Tool::Uv => cmd_version(dir, "uv", vec!["--version"], &version_regex),
            Tool::WitBindgenScalaJs => cmd_version(
                dir,
                "wit-bindgen-scalajs",
                vec!["--version"],
                &version_regex,
            ),
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
                            Cmp::Eq => Ok(VersionRelation::OkEqual),
                            Cmp::Lt => Ok(VersionRelation::KoOlder),
                            Cmp::Gt if newer_accepted => Ok(VersionRelation::OkNewer),
                            Cmp::Gt => Ok(VersionRelation::KoNewer),
                            _ => Err(format!("Unexpected compare result: {}", comp_op.sign())),
                        }
                    }
                    None => Err(format!("Failed to parse detected version: {version}")),
                },
                None => Err(format!(
                    "Failed to parse required version: {required_version}"
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
            Err(error) => Err(format!("Failed to get tool version: {error}")),
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

pub fn diagnose(dir: &Path, language: Option<GuestLanguage>) {
    let selected_language = match &language {
        Some(language) => SelectedLanguage::from_flag(dir, *language),
        None => SelectedLanguage::from_env(dir),
    };

    match &selected_language {
        Some(selected_language) => {
            match &selected_language.detected_by_reason {
                Some(reason) => {
                    logln(reason);
                    logln(format!(
                        "Detected language: {}",
                        selected_language.language.to_string().bold().green(),
                    ));
                }
                None => logln(format!("Selected language: {}", selected_language.language,)),
            }
            logln("Online language setup guide(s):");
            for url in selected_language.language.language_guide_setup_url() {
                logln(format!("  {}", url.bold().underline()));
            }
            logln("");

            report_tools(
                Tool::with_all_dependencies(selected_language.language.tools_with_rpc())
                    .iter()
                    .map(|t| DetectedTool::new(&selected_language.project_dir, *t))
                    .collect(),
            );
        }
        None => match &language {
            Some(language) => {
                logln(format!(
                    "The selected language ({language}) has no language specific diagnostics currently.\n",
                ));
                logln("Running diagnostics for common Worker to Worker RPC tooling.\n");
                report_tools(
                    Tool::with_all_dependencies(Language::common_rpc_tools())
                        .iter()
                        .map(|t| DetectedTool::new(dir, *t))
                        .collect(),
                );
            }
            None => {
                logln("No language detected");
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

    logln("Recommended tooling:");
    for tool in &all_tools {
        logln(format!(
            "  {: <width$} {}",
            format!("{}:", tool.metadata.short_name),
            tool.metadata.description,
            width = name_padding,
        ));
    }
    logln("");

    logln("Installed tool versions:");
    for tool in &all_tools {
        logln(format!(
            "  {: <name_padding$} {} {: <version_padding$}{}",
            format!("{}:", tool.metadata.short_name),
            tool.version_relation,
            tool.version.clone().unwrap_or_else(|| "".to_string()),
            tool.details,
        ));
    }
    logln("");

    let non_ok_tools: Vec<_> = all_tools
        .into_iter()
        .filter(|t| !t.version_relation.is_ok())
        .collect();

    if non_ok_tools.is_empty() {
        logln("All tools are ok.".green().to_string());
    } else {
        logln("Recommended steps:".yellow().to_string());
        for tool in &non_ok_tools {
            logln("");
            logln(format!(
                "  {}",
                format!(
                    "{}: {}",
                    tool.metadata.short_name, tool.metadata.description
                )
                .underline(),
            ));
            logln(format!(
                "    Problem: {} {}",
                tool.version_relation,
                tool.version
                    .clone()
                    .unwrap_or_else(|| tool.details.to_string()),
            ));
            logln("");
            logln("    Instructions:");
            for line in tool.metadata.instructions.lines() {
                logln(format!("      {}", line.yellow()));
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
                None => Err(format!("Failed to extract version from output: {output}")),
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
        if let Some(stripped) = s.strip_prefix("@") {
            stripped
        } else {
            s
        }
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

fn find_node_modules(dir: &Path) -> PathBuf {
    let node_modules = dir.join("node_modules");
    if node_modules.exists() {
        dir.to_path_buf()
    } else {
        let parent = dir.parent();
        if let Some(parent) = parent {
            find_node_modules(parent)
        } else {
            PathBuf::new()
        }
    }
}
