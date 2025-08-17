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

use fancy_regex::{Match, Regex};
use heck::{ToLowerCamelCase, ToPascalCase, ToSnakeCase, ToTitleCase};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::Formatter;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::LazyLock;
use std::{fmt, io};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ComponentName(String);

static COMPONENT_NAME_SPLIT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?=[A-Z\\-_:])").unwrap());

impl ComponentName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parts(&self) -> Vec<String> {
        let matches: Vec<Result<Match, fancy_regex::Error>> =
            COMPONENT_NAME_SPLIT_REGEX.find_iter(&self.0).collect();
        let mut parts: Vec<&str> = vec![];
        let mut last = 0;
        for m in matches.into_iter().flatten() {
            let part = &self.0[last..m.start()];
            if !part.is_empty() {
                parts.push(part);
            }
            last = m.end();
        }
        parts.push(&self.0[last..]);

        let mut result: Vec<String> = Vec::with_capacity(parts.len());
        for part in parts {
            let s = part.to_lowercase();
            let s = s.strip_prefix('-').unwrap_or(&s);
            let s = s.strip_prefix('_').unwrap_or(s);
            let s = s.strip_prefix(':').unwrap_or(s);
            result.push(s.to_string());
        }
        result
    }

    pub fn to_kebab_case(&self) -> String {
        self.parts().join("-")
    }

    pub fn to_snake_case(&self) -> String {
        self.parts().join("_")
    }

    pub fn to_pascal_case(&self) -> String {
        self.parts().iter().map(|s| s.to_title_case()).collect()
    }

    pub fn to_camel_case(&self) -> String {
        self.to_pascal_case().to_lower_camel_case()
    }
}

impl From<&str> for ComponentName {
    fn from(name: &str) -> Self {
        ComponentName(name.to_string())
    }
}

impl From<String> for ComponentName {
    fn from(name: String) -> Self {
        ComponentName(name)
    }
}

impl fmt::Display for ComponentName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TemplateKind {
    Standalone,
    ComposableAppCommon {
        group: ComposableAppGroupName,
        skip_if_exists: Option<PathBuf>,
    },
    ComposableAppComponent {
        group: ComposableAppGroupName,
    },
}

impl TemplateKind {
    pub fn is_common(&self) -> bool {
        matches!(self, TemplateKind::ComposableAppCommon { .. })
    }
}

#[derive(
    Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter, Serialize, Deserialize,
)]
pub enum GuestLanguage {
    Rust,
    TypeScript,
}

impl GuestLanguage {
    pub fn from_string(s: impl AsRef<str>) -> Option<GuestLanguage> {
        match s.as_ref().to_lowercase().as_str() {
            "rust" => Some(GuestLanguage::Rust),
            "ts" | "typescript" => Some(GuestLanguage::TypeScript),
            _ => None,
        }
    }

    pub fn id(&self) -> String {
        match self {
            GuestLanguage::Rust => "rust".to_string(),
            GuestLanguage::TypeScript => "ts".to_string(),
        }
    }

    pub fn tier(&self) -> GuestLanguageTier {
        match self {
            GuestLanguage::Rust => GuestLanguageTier::Tier1,
            GuestLanguage::TypeScript => GuestLanguageTier::Tier1,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            GuestLanguage::Rust => "Rust",
            GuestLanguage::TypeScript => "TypeScript",
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter, Serialize, Deserialize)]
pub enum GuestLanguageTier {
    Tier1,
    Tier2,
    Tier3,
}

impl GuestLanguageTier {
    pub fn from_string(s: impl AsRef<str>) -> Option<GuestLanguageTier> {
        match s.as_ref().to_lowercase().as_str() {
            "tier1" | "1" => Some(GuestLanguageTier::Tier1),
            "tier2" | "2" => Some(GuestLanguageTier::Tier2),
            "tier3" | "3" => Some(GuestLanguageTier::Tier3),
            _ => None,
        }
    }

    pub fn level(&self) -> u8 {
        match self {
            GuestLanguageTier::Tier1 => 1,
            GuestLanguageTier::Tier2 => 2,
            GuestLanguageTier::Tier3 => 3,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            GuestLanguageTier::Tier1 => "tier1",
            GuestLanguageTier::Tier2 => "tier2",
            GuestLanguageTier::Tier3 => "tier3",
        }
    }
}

impl fmt::Display for GuestLanguageTier {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for GuestLanguageTier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        GuestLanguageTier::from_string(s).ok_or(format!("Unexpected guest language tier {s}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PackageName((String, String));

impl PackageName {
    pub fn from_string(s: impl AsRef<str>) -> Option<PackageName> {
        let parts: Vec<&str> = s.as_ref().split(':').collect();
        match parts.as_slice() {
            &[n1, n2] if !n1.is_empty() && !n2.is_empty() => {
                Some(PackageName((n1.to_string(), n2.to_string())))
            }
            _ => None,
        }
    }

    pub fn to_pascal_case(&self) -> String {
        format!(
            "{}{}",
            self.0 .0.to_pascal_case(),
            self.0 .1.to_pascal_case()
        )
    }

    pub fn to_snake_case(&self) -> String {
        format!(
            "{}_{}",
            self.0 .0.to_snake_case(),
            self.0 .1.to_snake_case()
        )
    }

    pub fn to_string_with_double_colon(&self) -> String {
        format!("{}::{}", self.0 .0, self.0 .1)
    }

    pub fn to_string_with_colon(&self) -> String {
        format!("{}:{}", self.0 .0, self.0 .1)
    }

    pub fn to_string_with_slash(&self) -> String {
        format!("{}/{}", self.0 .0, self.0 .1)
    }

    pub fn to_kebab_case(&self) -> String {
        format!("{}-{}", self.0 .0, self.0 .1)
    }

    pub fn to_rust_binding(&self) -> String {
        format!(
            "{}::{}",
            self.0 .0.to_snake_case(),
            self.0 .1.to_snake_case()
        )
    }

    pub fn namespace(&self) -> String {
        self.0 .0.to_string()
    }

    pub fn namespace_title_case(&self) -> String {
        self.0 .0.to_title_case()
    }

    pub fn namespace_snake_case(&self) -> String {
        self.0 .0.to_snake_case()
    }

    pub fn name_snake_case(&self) -> String {
        self.0 .1.to_snake_case()
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_with_colon())
    }
}

impl FromStr for PackageName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PackageName::from_string(s).ok_or(format!(
            "Unexpected package name {s}. Must be in 'pack:name' format"
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TemplateName(String);

impl TemplateName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TemplateName {
    fn from(s: &str) -> Self {
        TemplateName(s.to_string())
    }
}

impl From<String> for TemplateName {
    fn from(s: String) -> Self {
        TemplateName(s)
    }
}

impl fmt::Display for TemplateName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ComposableAppGroupName(String);

impl ComposableAppGroupName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ComposableAppGroupName {
    fn default() -> Self {
        ComposableAppGroupName("default".to_string())
    }
}

impl From<&str> for ComposableAppGroupName {
    fn from(s: &str) -> Self {
        ComposableAppGroupName(s.to_string())
    }
}

impl From<String> for ComposableAppGroupName {
    fn from(s: String) -> Self {
        ComposableAppGroupName(s)
    }
}

impl fmt::Display for ComposableAppGroupName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Copy, Clone)]
pub enum TargetExistsResolveMode {
    Skip,
    MergeOrSkip,
    Fail,
    MergeOrFail,
}

pub type MergeContents = Box<dyn FnOnce(&[u8]) -> io::Result<Vec<u8>>>;

pub enum TargetExistsResolveDecision {
    Skip,
    Merge(MergeContents),
}

#[derive(Debug, Clone)]
pub struct Template {
    pub name: TemplateName,
    pub kind: TemplateKind,
    pub language: GuestLanguage,
    pub description: String,
    pub template_path: PathBuf,
    pub instructions: String,
    pub wit_deps: Vec<PathBuf>,
    pub wit_deps_targets: Option<Vec<PathBuf>>,
    pub exclude: HashSet<String>,
    pub transform_exclude: HashSet<String>,
    pub transform: bool,
}

#[derive(Debug, Clone)]
pub struct TemplateParameters {
    pub component_name: ComponentName,
    pub package_name: PackageName,
    pub target_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TemplateMetadata {
    pub description: String,
    #[serde(rename = "appCommonGroup")]
    pub app_common_group: Option<String>,
    #[serde(rename = "appCommonSkipIfExists")]
    pub app_common_skip_if_exists: Option<String>,
    #[serde(rename = "appComponentGroup")]
    pub app_component_group: Option<String>,
    #[serde(rename = "requiresGolemHostWIT")]
    pub requires_golem_host_wit: Option<bool>,
    #[serde(rename = "requiresWASI")]
    pub requires_wasi: Option<bool>,
    #[serde(rename = "witDepsPaths")]
    pub wit_deps_paths: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub instructions: Option<String>,
    #[serde(rename = "transformExclude")]
    pub transform_exclude: Option<Vec<String>>,
    pub transform: Option<bool>,
}

#[cfg(test)]
mod tests {
    use crate::model::{ComponentName, PackageName};
    use test_r::test;

    #[allow(dead_code)]
    fn n1() -> ComponentName {
        "my-test-component".into()
    }

    #[allow(dead_code)]
    fn n2() -> ComponentName {
        "MyTestComponent".into()
    }

    #[allow(dead_code)]
    fn n3() -> ComponentName {
        "myTestComponent".into()
    }

    #[allow(dead_code)]
    fn n4() -> ComponentName {
        "my_test_component".into()
    }

    #[test]
    pub fn component_name_to_pascal_case() {
        assert_eq!(n1().to_pascal_case(), "MyTestComponent");
        assert_eq!(n2().to_pascal_case(), "MyTestComponent");
        assert_eq!(n3().to_pascal_case(), "MyTestComponent");
        assert_eq!(n4().to_pascal_case(), "MyTestComponent");
    }

    #[test]
    pub fn component_name_to_camel_case() {
        assert_eq!(n1().to_camel_case(), "myTestComponent");
        assert_eq!(n2().to_camel_case(), "myTestComponent");
        assert_eq!(n3().to_camel_case(), "myTestComponent");
        assert_eq!(n4().to_camel_case(), "myTestComponent");
    }

    #[test]
    pub fn component_name_to_snake_case() {
        assert_eq!(n1().to_snake_case(), "my_test_component");
        assert_eq!(n2().to_snake_case(), "my_test_component");
        assert_eq!(n3().to_snake_case(), "my_test_component");
        assert_eq!(n4().to_snake_case(), "my_test_component");
    }

    #[test]
    pub fn component_name_to_kebab_case() {
        assert_eq!(n1().to_kebab_case(), "my-test-component");
        assert_eq!(n2().to_kebab_case(), "my-test-component");
        assert_eq!(n3().to_kebab_case(), "my-test-component");
        assert_eq!(n4().to_kebab_case(), "my-test-component");
    }

    #[allow(dead_code)]
    fn p1() -> PackageName {
        PackageName::from_string("foo:bar").unwrap()
    }

    #[allow(dead_code)]
    fn p2() -> PackageName {
        PackageName::from_string("foo:bar-baz").unwrap()
    }

    #[test]
    pub fn package_name_to_pascal_case() {
        assert_eq!(p1().to_pascal_case(), "FooBar");
        assert_eq!(p2().to_pascal_case(), "FooBarBaz");
    }

    #[test]
    pub fn package_name_with_number() {
        assert_eq!(
            PackageName::from_string("example:demo1")
                .unwrap()
                .to_rust_binding(),
            "example::demo1"
        )
    }
}
