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

use crate::model::GuestLanguage;

#[derive(Clone, Copy, Debug)]
pub struct VersionRange {
    pub min_inclusive: Option<&'static str>,
    pub max_exclusive: Option<&'static str>,
}

impl VersionRange {
    pub const fn at_least(min_inclusive: &'static str) -> Self {
        Self {
            min_inclusive: Some(min_inclusive),
            max_exclusive: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ToolRequirementCheck {
    CommandVersion {
        command: &'static str,
        args: &'static [&'static str],
    },
    RustTargetInstalled {
        target: &'static str,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct ToolRequirement {
    pub key: &'static str,
    pub name: &'static str,
    pub check: ToolRequirementCheck,
    pub version_range: Option<VersionRange>,
    pub install_hint: &'static str,
}

const RUST_TOOL_REQUIREMENTS: &[ToolRequirement] = &[
    ToolRequirement {
        key: "rustup",
        name: "rustup",
        check: ToolRequirementCheck::CommandVersion {
            command: "rustup",
            args: &["--version"],
        },
        version_range: Some(VersionRange::at_least("1.27.1")),
        install_hint: "Install Rust tooling using rustup: https://www.rust-lang.org/tools/install",
    },
    ToolRequirement {
        key: "rustc",
        name: "rustc",
        check: ToolRequirementCheck::CommandVersion {
            command: "rustc",
            args: &["--version"],
        },
        version_range: Some(VersionRange::at_least("1.94.0")),
        install_hint:
            "Install stable Rust with rustup: rustup install stable && rustup default stable",
    },
    ToolRequirement {
        key: "cargo",
        name: "cargo",
        check: ToolRequirementCheck::CommandVersion {
            command: "cargo",
            args: &["version"],
        },
        version_range: Some(VersionRange::at_least("1.94.0")),
        install_hint: "Cargo is installed with Rust toolchain from rustup",
    },
    ToolRequirement {
        key: "rust-target-wasm32-wasip2",
        name: "rust target wasm32-wasip2",
        check: ToolRequirementCheck::RustTargetInstalled {
            target: "wasm32-wasip2",
        },
        version_range: None,
        install_hint: "Install the Rust target: rustup target add wasm32-wasip2",
    },
];

const TYPESCRIPT_TOOL_REQUIREMENTS: &[ToolRequirement] = &[
    ToolRequirement {
        key: "node",
        name: "node",
        check: ToolRequirementCheck::CommandVersion {
            command: "node",
            args: &["--version"],
        },
        version_range: Some(VersionRange::at_least("24.11.0")),
        install_hint: "Install Node.js: https://nodejs.org/",
    },
    ToolRequirement {
        key: "npm",
        name: "npm",
        check: ToolRequirementCheck::CommandVersion {
            command: "npm",
            args: &["--version"],
        },
        version_range: Some(VersionRange::at_least("11.6.2")),
        install_hint: "npm is installed with Node.js",
    },
];

pub fn tool_requirements_for_language(language: GuestLanguage) -> &'static [ToolRequirement] {
    match language {
        GuestLanguage::Rust => RUST_TOOL_REQUIREMENTS,
        GuestLanguage::TypeScript => TYPESCRIPT_TOOL_REQUIREMENTS,
    }
}
