// Copyright 2024 Golem Cloud
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

use crate::diagnose::VersionRequirement::{ExactVersion, MinimumVersion, TodoVersion};
use regex::Regex;
use std::cmp::max;
use std::collections::HashSet;
use std::fmt::Display;
use std::fmt::Formatter;
use std::path::{Path, PathBuf};
use std::process::Command;
use version_compare::{CompOp, Version};
use walkdir::DirEntry;

#[derive(PartialEq, Eq, Hash, Copy, Clone)]
enum Langugage {
    CCcp,
    Go,
    JsTs,
    Python,
    Rust,
    Zig,
}

struct DetectedLanguage {
    language: Langugage,
    project_dir: PathBuf,
    reason: String,
}

impl Langugage {
    pub fn detect() -> Option<DetectedLanguage> {
        let ordered_language_project_hint_files: Vec<(&str, Langugage)> = vec![
            ("build.zig", Langugage::Zig),
            ("Cargo.toml", Langugage::Rust),
            ("go.mod", Langugage::Go),
            ("package.json", Langugage::JsTs),
            ("Makefile", Langugage::CCcp),
            ("main.py", Langugage::Python),
        ];

        let detect_in_dir = |dir: &Path| -> Option<DetectedLanguage> {
            ordered_language_project_hint_files
                .iter()
                .find_map(|(file, language)| {
                    let path = dir.join(Path::new(file));
                    path.exists().then_some(DetectedLanguage {
                        language: *language,
                        project_dir: path
                            .parent()
                            .expect("Failed to get parent for project file")
                            .to_path_buf(),
                        reason: format!("Detected project file: {}", path.to_string_lossy()),
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

    pub fn tools(&self) -> Vec<Tool> {
        match self {
            Langugage::CCcp => vec![
                Tool::Clang,
                Tool::WasiSdk,
                Tool::WitBindgen,
                Tool::WasmTools,
            ],
            Langugage::Go => vec![
                Tool::TinyGo,
                Tool::WitBindgen,
                Tool::WasmTools,
                Tool::GolemSdkGo,
            ],
            Langugage::JsTs => vec![
                Tool::Npm,
                Tool::Jco,
                Tool::ComponentizeJs,
                Tool::GolemSdkTypeScript,
            ],
            Langugage::Python => vec![Tool::ComponentizePy],
            Langugage::Rust => vec![Tool::RustTargetWasm32Wasi, Tool::CargoComponent],
            Langugage::Zig => vec![Tool::Zig, Tool::WitBindgen, Tool::WasmTools],
        }
    }

    pub fn language_guide_setup_url(&self) -> Vec<&str> {
        match self {
            Langugage::CCcp => vec!["https://learn.golem.cloud/docs/ccpp-language-guide/setup"],
            Langugage::Go => vec!["https://learn.golem.cloud/docs/go-language-guide/setup"],
            Langugage::JsTs => vec![
                "https://learn.golem.cloud/docs/experimental-languages/js-language-guide/setup",
                "https://learn.golem.cloud/docs/experimental-languages/ts-language-guide/setup",
            ],
            Langugage::Python => vec!["https://learn.golem.cloud/docs/python-language-guide/setup"],
            Langugage::Rust => vec!["https://learn.golem.cloud/docs/rust-language-guide/setup"],
            Langugage::Zig => vec![
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

impl Display for Langugage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Langugage::CCcp => f.write_str("C or C++"),
            Langugage::Go => f.write_str("Go"),
            Langugage::JsTs => f.write_str("JavaScript or TypeScript"),
            Langugage::Python => f.write_str("Python"),
            Langugage::Rust => f.write_str("Rust"),
            Langugage::Zig => f.write_str("Zig"),
        }
    }
}

#[allow(clippy::enum_variant_names)]
enum VersionRequirement {
    ExactVersion(&'static str),
    MinimumVersion(&'static str),
    TodoVersion, // TODO
}

impl VersionRequirement {
    pub fn to_parsed_version(&self) -> Version {
        match self {
            ExactVersion(version) => Version::from(version).expect("Failed to parse exact version"),
            MinimumVersion(version) => {
                Version::from(version).expect("Failed to parse minimum version")
            }
            TodoVersion => Version::from("0.0.0.0").expect("TODO version"),
        }
    }
}

enum VersionRelation {
    OkEqual,
    OkNewer,
    KoNewer,
    KoOlder,
    Missing,
}

impl Display for VersionRelation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionRelation::OkEqual => f.write_str("equal:   ok"),
            VersionRelation::OkNewer => f.write_str("newer:   ok"),
            VersionRelation::KoNewer => f.write_str("newer:   !!"),
            VersionRelation::KoOlder => f.write_str("older:   !!"),
            VersionRelation::Missing => f.write_str("missing: !!"),
        }
    }
}

struct ToolMetadata {
    pub short_name: &'static str,
    pub description: &'static str,
    pub version_requirement: VersionRequirement,
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
enum Tool {
    Cargo,
    CargoComponent,
    Clang,
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
            },
            Tool::CargoComponent => ToolMetadata {
                short_name: "cargo-component",
                description: "Cargo subcommand for building WebAssembly components",
                version_requirement: ExactVersion("0.13.2"),
            },
            Tool::Clang => ToolMetadata {
                short_name: "clang",
                description: "C and C++ compiler",
                version_requirement: TodoVersion,
            },
            Tool::ComponentizeJs => ToolMetadata {
                short_name: "componentize-js",
                description:
                    "Tool for converting JavaScript applications to WebAssembly components",
                version_requirement: MinimumVersion("0.10.5-golem.3"),
            },
            Tool::ComponentizePy => ToolMetadata {
                short_name: "componentize-py",
                description: "Tool for converting Python applications to WebAssembly components",
                version_requirement: ExactVersion("0.13.5"),
            },
            Tool::Go => ToolMetadata {
                short_name: "go",
                description: "Go language tooling",
                version_requirement: MinimumVersion("1.20.0"),
            },
            Tool::GolemSdkGo => ToolMetadata {
                short_name: "golem-go",
                description: "Golem SDK for Go",
                version_requirement: MinimumVersion("0.7.0"),
            },
            Tool::GolemSdkRust => ToolMetadata {
                short_name: "golem-rust",
                description: "Golem SDK for Rust",
                version_requirement: TodoVersion,
            },
            Tool::GolemSdkTypeScript => ToolMetadata {
                short_name: "golem-ts",
                description: "Golem SDK for JavaScript and TypeScript",
                version_requirement: MinimumVersion("0.2.0"),
            },
            Tool::Jco => ToolMetadata {
                short_name: "jco",
                description: "Toolchain for working with WebAssembly Components in JavaScript",
                version_requirement: MinimumVersion("1.4.4-golem.1"),
            },
            Tool::Node => ToolMetadata {
                short_name: "node",
                description: "JavaScript runtime",
                version_requirement: MinimumVersion("20.17.0"),
            },
            Tool::Npm => ToolMetadata {
                short_name: "npm",
                description: "Node package manager",
                version_requirement: MinimumVersion("10.8.2"),
            },
            Tool::Pip => ToolMetadata {
                short_name: "pip",
                description: "Python package installer",
                version_requirement: MinimumVersion("24.0"),
            },
            Tool::Python => ToolMetadata {
                short_name: "python",
                description: "Python interpreter",
                version_requirement: MinimumVersion("3.10"),
            },
            Tool::Rustc => ToolMetadata {
                short_name: "rustc",
                description: "Rust compiler",
                version_requirement: MinimumVersion("1.80.1"),
            },
            Tool::Rustup => ToolMetadata {
                short_name: "rustup",
                description: "Rust toolchain installer",
                version_requirement: MinimumVersion("1.27.1"),
            },
            Tool::RustTargetWasm32Wasi => ToolMetadata {
                short_name: "rust target wasm32-wasi",
                description: "Rust target for building WebAssembly components",
                version_requirement: TodoVersion,
            },
            Tool::TinyGo => ToolMetadata {
                short_name: "tinygo",
                description: "Go compiler for WebAssembly (and embedded systems)",
                version_requirement: MinimumVersion("0.33"),
            },
            Tool::WasiSdk => ToolMetadata {
                short_name: "wasi-sdk",
                description: "WebAssembly toolchain for C and C++",
                version_requirement: TodoVersion,
            },
            Tool::WasmTools => ToolMetadata {
                short_name: "wasm-tools",
                description: "Tools for manipulation of WebAssembly modules",
                version_requirement: ExactVersion("1.210.0"),
            },
            Tool::WitBindgen => ToolMetadata {
                short_name: "wit-bindgen",
                description: "Guest language bindings generator for WIT",
                version_requirement: ExactVersion("0.26.0"),
            },
            Tool::Zig => ToolMetadata {
                short_name: "zig",
                description: "Zig language tooling",
                version_requirement: MinimumVersion("0.13.0"),
            },
        }
    }

    pub fn direct_dependencies(&self) -> Vec<Tool> {
        match self {
            Tool::Cargo => vec![Tool::Rustup],
            Tool::CargoComponent => vec![Tool::Cargo],
            Tool::Clang => vec![],
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
            Tool::Clang => cmd_version(dir, "clang", vec!["--version"], &version_regex),
            Tool::ComponentizeJs => npm_package_version(dir, "@golemcloud/componentize-js"),
            Tool::ComponentizePy => Err("TODO".to_string()),
            Tool::Go => cmd_version(dir, "go", vec!["version"], &version_regex),
            Tool::GolemSdkGo => go_mod_version(dir, "github.com/golemcloud/golem-go"),
            Tool::GolemSdkRust => Err("TODO".to_string()),
            Tool::GolemSdkTypeScript => npm_package_version(dir, "@golemcloud/golem-ts"),
            Tool::Jco => npm_package_version(dir, "@golemcloud/jco"),
            Tool::Node => cmd_version(dir, "node", vec!["--version"], &version_regex),
            Tool::Npm => cmd_version(dir, "npm", vec!["--version"], &version_regex),
            Tool::Pip => cmd_version(dir, "pip", vec!["--version"], &version_regex),
            Tool::Python => cmd_version(dir, "python", vec!["--version"], &version_regex),
            Tool::Rustc => cmd_version(dir, "rustc", vec!["--version"], &version_regex),
            Tool::Rustup => cmd_version(dir, "rustup", vec!["--version"], &version_regex),
            Tool::RustTargetWasm32Wasi => Err("TODO".to_string()),
            Tool::TinyGo => cmd_version(dir, "tinygo", vec!["version"], &version_regex),
            Tool::WasiSdk => Err("TODO".to_string()),
            Tool::WasmTools => cmd_version(dir, "wasm-tools", vec!["--version"], &version_regex),
            Tool::WitBindgen => cmd_version(dir, "wit-bindgen", vec!["--version"], &version_regex),
            Tool::Zig => cmd_version(dir, "zig", vec!["version"], &version_regex),
        }
    }
}

pub fn diagnose() {
    match Langugage::detect() {
        Some(detected_language) => {
            println!("{}", detected_language.reason);
            println!(
                "Detected language: {} (to explicitly specify the language use the --language flag)",
                detected_language.language,
            );
            println!("Online language setup guide(s):");
            for url in detected_language.language.language_guide_setup_url() {
                println!("  {url}");
            }
            println!();

            let all_tools: Vec<_> =
                Tool::with_all_dependencies(detected_language.language.tools_with_rpc())
                    .iter()
                    .map(|t| (*t, t.metadata()))
                    .map(|(tool, meta)| {
                        (tool, meta, tool.get_version(&detected_language.project_dir))
                    })
                    .collect();

            let (name_padding, version_padding) = {
                let mut name_padding = 0;
                let mut version_padding = 0;
                for (_, meta, version) in &all_tools {
                    name_padding = max(name_padding, meta.short_name.len());
                    version_padding = max(
                        version_padding,
                        version.as_ref().ok().map(|v| v.len()).unwrap_or(0),
                    );
                }
                (name_padding + 1, version_padding + 1)
            };

            println!("Recommended tooling:");
            for (_, metadata, _) in &all_tools {
                println!(
                    "  {: <width$} {}",
                    format!("{}:", metadata.short_name),
                    metadata.description,
                    width = name_padding,
                );
            }
            println!();

            println!("Installed tool versions:");
            for (_, metadata, version) in &all_tools {
                let required_version = metadata.version_requirement.to_parsed_version();

                let version = match version {
                    Ok(version) => Version::from(version)
                        .ok_or_else(|| format!("Failed to parse version: {}", version)),
                    Err(err) => Err(err.clone()),
                };

                let comp_op = version.as_ref().ok().map(|v| v.compare(&required_version));
                let version_relation = match (&comp_op, &metadata.version_requirement) {
                    (Some(CompOp::Eq), _) => VersionRelation::OkEqual,
                    (Some(CompOp::Gt), MinimumVersion(_)) => VersionRelation::OkNewer,
                    (Some(CompOp::Lt), _) => VersionRelation::KoOlder,
                    (Some(CompOp::Gt), ExactVersion(_)) => VersionRelation::KoNewer,
                    _ => VersionRelation::Missing,
                };

                println!(
                    "  {: <name_padding$} [{}] {: <version_padding$}{}",
                    format!("{}:", metadata.short_name),
                    version_relation,
                    match version {
                        Ok(version) => version.to_string(),
                        Err(error) => format!("{: <version_padding$} ({})", "", error).to_string(),
                    },
                    comp_op
                        .map(|c| format!(" ({}{})", c.sign(), required_version))
                        .unwrap_or_else(|| "".to_string()),
                );
            }
        }
        None => {
            println!("No language detected, use the --language flag to explicitly specify one");
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
        let tokens: Vec<_> = dep.split(" ").collect();
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
