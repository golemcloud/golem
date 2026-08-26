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

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Formatter};
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

// NOTE: the order of languages (currently) is NOT alphabetical, rather based on recommendation
#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    EnumIter,
    Serialize,
    Deserialize,
    ValueEnum,
)]
#[clap(rename_all = "lower")]
pub enum GuestLanguage {
    #[value(alias = "ts")]
    TypeScript,
    Rust,
    Scala,
    MoonBit,
}

impl GuestLanguage {
    pub fn from_string(s: impl AsRef<str>) -> Option<GuestLanguage> {
        match s.as_ref().to_lowercase().as_str() {
            "rust" => Some(GuestLanguage::Rust),
            "ts" | "typescript" => Some(GuestLanguage::TypeScript),
            "scala" => Some(GuestLanguage::Scala),
            "moonbit" => Some(GuestLanguage::MoonBit),
            _ => None,
        }
    }

    pub fn from_id_string(s: impl AsRef<str>) -> Option<GuestLanguage> {
        match s.as_ref().to_lowercase().as_str() {
            "rust" => Some(GuestLanguage::Rust),
            "ts" => Some(GuestLanguage::TypeScript),
            "scala" => Some(GuestLanguage::Scala),
            "moonbit" => Some(GuestLanguage::MoonBit),
            _ => None,
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            GuestLanguage::Rust => "rust",
            GuestLanguage::TypeScript => "ts",
            GuestLanguage::Scala => "scala",
            GuestLanguage::MoonBit => "moonbit",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            GuestLanguage::Rust => "Rust",
            GuestLanguage::TypeScript => "TypeScript",
            GuestLanguage::Scala => "Scala",
            GuestLanguage::MoonBit => "MoonBit",
        }
    }
}

impl fmt::Display for GuestLanguage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for GuestLanguage {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        GuestLanguage::from_string(s).ok_or({
            let all = GuestLanguage::iter()
                .map(|x| format!("\"{x}\""))
                .collect::<Vec<String>>()
                .join(", ");
            format!("Unknown guest language: {s}. Expected one of {all}")
        })
    }
}
