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

mod agent;
mod component;
mod deployment;
mod environment;
mod hash;
mod http_api_deployment;
mod mcp_deployment;
mod plugin;
mod resource_definition;
mod ser;

pub use agent::*;
pub use crate::base_model::diff::DIFF_MODEL_VERSION;
pub use component::*;
pub use deployment::*;
pub use environment::*;
pub use hash::*;
pub use http_api_deployment::*;
pub use mcp_deployment::*;
pub use plugin::*;
pub use resource_definition::*;
pub use ser::*;

use serde::{Serialize, Serializer};
use similar::TextDiff;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub trait Diffable {
    type DiffResult: Serialize;

    fn diff_with_new(&self, new: &Self) -> Option<Self::DiffResult> {
        Self::diff(new, self)
    }

    fn diff_with_current(&self, current: &Self) -> Option<Self::DiffResult> {
        Self::diff(self, current)
    }

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult>;

    fn unified_yaml_diff_with_new(&self, new: &Self, mode: SerializeMode) -> String
    where
        Self: Serialize,
    {
        Self::unified_yaml_diff(new, self, mode)
    }

    fn unified_yaml_diff_with_current(&self, current: &Self, mode: SerializeMode) -> String
    where
        Self: Serialize,
    {
        Self::unified_yaml_diff(self, current, mode)
    }

    fn unified_yaml_diff(new: &Self, current: &Self, mode: SerializeMode) -> String
    where
        Self: Serialize,
    {
        unified_diff(
            to_yaml_with_mode(&current, mode).expect("failed to serialize current"),
            to_yaml_with_mode(&new, mode).expect("failed to serialize current"),
        )
    }
}

pub trait VecDiffable: Diffable {
    type OrderingKey: Ord;

    fn ordering_key(&self) -> Self::OrderingKey;
}

pub fn unified_diff(current: impl AsRef<str>, new: impl AsRef<str>) -> String {
    TextDiff::from_lines(current.as_ref(), new.as_ref())
        .unified_diff()
        .context_radius(4)
        .to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub enum BTreeMapDiffValue<ValueDiff> {
    Create,
    Delete,
    Update(ValueDiff),
}

impl<ValueDiff: Serialize> Serialize for BTreeMapDiffValue<ValueDiff> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            BTreeMapDiffValue::Create => serializer.serialize_str("create"),
            BTreeMapDiffValue::Delete => serializer.serialize_str("delete"),
            BTreeMapDiffValue::Update(diff) => diff.serialize(serializer),
        }
    }
}

pub type BTreeMapDiff<K, V> = BTreeMap<K, BTreeMapDiffValue<<V as Diffable>::DiffResult>>;

impl<K, V> Diffable for BTreeMap<K, V>
where
    K: Ord + Clone + Serialize,
    V: Diffable,
    V::DiffResult: Serialize,
{
    type DiffResult = BTreeMapDiff<K, V>;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let mut diff = BTreeMap::new();

        let keys = new.keys().chain(current.keys()).collect::<BTreeSet<_>>();

        for key in keys {
            match (new.get(key), current.get(key)) {
                (Some(new), Some(current)) => {
                    if let Some(value_diff) = new.diff_with_current(current) {
                        diff.insert(key.clone(), BTreeMapDiffValue::Update(value_diff));
                    }
                }
                (Some(_), None) => {
                    diff.insert(key.clone(), BTreeMapDiffValue::Create);
                }
                (None, Some(_)) => {
                    diff.insert(key.clone(), BTreeMapDiffValue::Delete);
                }
                (None, None) => {
                    unreachable!("key must be present either in new or current");
                }
            }
        }

        if diff.is_empty() { None } else { Some(diff) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BTreeSetDiffValue {
    Create,
    Delete,
}

pub type BTreeSetDiff<K> = BTreeMap<K, BTreeSetDiffValue>;

impl<K> Diffable for BTreeSet<K>
where
    K: Ord + Clone + Serialize,
{
    type DiffResult = BTreeSetDiff<K>;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let mut diff = BTreeMap::new();

        let keys = new.iter().chain(current.iter()).collect::<BTreeSet<_>>();

        for key in keys {
            match (new.contains(key), current.contains(key)) {
                (true, true) => {
                    // NOP, same
                }
                (true, false) => {
                    diff.insert(key.clone(), BTreeSetDiffValue::Create);
                }
                (false, true) => {
                    diff.insert(key.clone(), BTreeSetDiffValue::Delete);
                }
                (false, false) => {
                    panic!("unreachable");
                }
            }
        }

        if diff.is_empty() { None } else { Some(diff) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VecDiffValue<OrderingKey, ValueDiff> {
    Create(OrderingKey),
    Delete(OrderingKey),
    Update(OrderingKey, ValueDiff),
}

pub type VecDiff<V> =
    Vec<VecDiffValue<<V as VecDiffable>::OrderingKey, <V as Diffable>::DiffResult>>;

impl<V> Diffable for Vec<V>
where
    V: VecDiffable,
    V::OrderingKey: Serialize,
    V::DiffResult: Serialize,
{
    type DiffResult = VecDiff<V>;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let mut diff = Vec::new();

        let mut new: VecDeque<(V::OrderingKey, &V)> = new
            .iter()
            .map(|v| (v.ordering_key(), v))
            .collect::<Vec<_>>()
            .into();

        assert!(new.iter().is_sorted_by_key(|(k, _)| k));

        let mut current: VecDeque<(V::OrderingKey, &V)> = current
            .iter()
            .map(|v| (v.ordering_key(), v))
            .collect::<Vec<_>>()
            .into();

        assert!(current.iter().is_sorted_by_key(|(k, _)| k));

        while !new.is_empty() || !current.is_empty() {
            match (new.front(), current.front()) {
                (Some((kn, _)), Some((kc, _))) => match kn.cmp(kc) {
                    std::cmp::Ordering::Equal => {
                        let (kn, n) = new.pop_front().unwrap();
                        let (_, c) = current.pop_front().unwrap();

                        if let Some(d) = n.diff_with_current(c) {
                            diff.push(VecDiffValue::Update(kn, d));
                        }
                    }
                    std::cmp::Ordering::Less => {
                        let (k, _) = new.pop_front().unwrap();
                        diff.push(VecDiffValue::Create(k));
                    }
                    std::cmp::Ordering::Greater => {
                        let (k, _) = current.pop_front().unwrap();
                        diff.push(VecDiffValue::Delete(k));
                    }
                },
                (Some(_), None) => {
                    let (k, _) = new.pop_front().unwrap();
                    diff.push(VecDiffValue::Create(k));
                }
                (None, Some(_)) => {
                    let (k, _) = current.pop_front().unwrap();
                    diff.push(VecDiffValue::Delete(k));
                }
                (None, None) => break,
            }
        }

        if diff.is_empty() { None } else { Some(diff) }
    }
}

/// A normalized JSON value changed: the diff result is the new value itself.
impl Diffable for crate::base_model::json::NormalizedJsonValue {
    type DiffResult = crate::base_model::json::NormalizedJsonValue;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        if new != current {
            Some(new.clone())
        } else {
            None
        }
    }
}

/// A string value changed: the diff result is the new string.
impl Diffable for String {
    type DiffResult = String;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        if new != current {
            Some(new.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DIFF_MODEL_VERSION;
    use quote::ToTokens;
    use std::fs;
    use std::path::{Path, PathBuf};
    use test_r::test;

    #[test]
    fn diff_model_version_matches_diff_module_fingerprint() {
        let expected = expected_diff_model_fingerprint(DIFF_MODEL_VERSION).unwrap_or_else(|| {
            panic!(
                "No expected diff model fingerprint configured for DIFF_MODEL_VERSION={DIFF_MODEL_VERSION}. Add one in expected_diff_model_fingerprint."
            )
        });

        let actual = diff_module_fingerprint();

        assert_eq!(
            actual,
            expected,
            "Diff module fingerprint changed for DIFF_MODEL_VERSION={DIFF_MODEL_VERSION}. \
Bump DIFF_MODEL_VERSION if the model change is breaking; otherwise update expected_diff_model_fingerprint."
        );
    }

    fn diff_module_fingerprint() -> String {
        let mut hasher = blake3::Hasher::new();

        for path in diff_module_source_files() {
            let content = fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("Failed to read {}: {err}", path.display()));

            let syntax = syn::parse_file(&content)
                .unwrap_or_else(|err| panic!("Failed to parse {}: {err}", path.display()));
            let normalized = strip_test_only_items(syntax);

            let relative_path = path
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .expect("diff source file path should be inside crate root");

            hasher.update(relative_path.to_string_lossy().as_bytes());
            hasher.update(&[0]);
            hasher.update(normalized.into_token_stream().to_string().as_bytes());
            hasher.update(&[0]);
        }

        hasher.finalize().to_hex().to_string()
    }

    fn diff_module_source_files() -> Vec<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();

        for rel_dir in ["src/model/diff", "src/base_model/diff"] {
            let dir = root.join(rel_dir);
            let mut dir_files = fs::read_dir(&dir)
                .unwrap_or_else(|err| panic!("Failed to list {}: {err}", dir.display()))
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
                .collect::<Vec<_>>();

            files.append(&mut dir_files);
        }

        files.sort();
        files
    }

    fn expected_diff_model_fingerprint(version: u32) -> Option<&'static str> {
        match version {
            1 => Some("ba98c5062c5db3022d3687ce8c135c2a2326c7711955e3b90d0c8ed63a66af9c"),
            _ => None,
        }
    }

    fn strip_test_only_items(file: syn::File) -> syn::File {
        let attrs = file
            .attrs
            .into_iter()
            .filter(|attr| !is_cfg_test_attr(attr))
            .collect();

        let items = file
            .items
            .into_iter()
            .filter(|item| !item_attrs(item).iter().any(is_cfg_test_attr))
            .collect();

        syn::File {
            shebang: file.shebang,
            attrs,
            items,
        }
    }

    fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
        match item {
            syn::Item::Const(item) => &item.attrs,
            syn::Item::Enum(item) => &item.attrs,
            syn::Item::ExternCrate(item) => &item.attrs,
            syn::Item::Fn(item) => &item.attrs,
            syn::Item::ForeignMod(item) => &item.attrs,
            syn::Item::Impl(item) => &item.attrs,
            syn::Item::Macro(item) => &item.attrs,
            syn::Item::Mod(item) => &item.attrs,
            syn::Item::Static(item) => &item.attrs,
            syn::Item::Struct(item) => &item.attrs,
            syn::Item::Trait(item) => &item.attrs,
            syn::Item::TraitAlias(item) => &item.attrs,
            syn::Item::Type(item) => &item.attrs,
            syn::Item::Union(item) => &item.attrs,
            syn::Item::Use(item) => &item.attrs,
            _ => &[],
        }
    }

    fn is_cfg_test_attr(attr: &syn::Attribute) -> bool {
        if !attr.path().is_ident("cfg") {
            return false;
        }

        let mut found = false;

        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("test") {
                found = true;
            }
            Ok(())
        });

        found
    }
}
