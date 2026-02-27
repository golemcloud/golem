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

mod component;
mod deployment;
mod environment;
mod hash;
mod http_api_deployment;
mod plugin;
mod ser;

pub use component::*;
pub use deployment::*;
pub use environment::*;
pub use hash::*;
pub use http_api_deployment::*;
pub use plugin::*;
pub use ser::*;

use serde::{Serialize, Serializer};
use similar::TextDiff;
use std::collections::{BTreeMap, BTreeSet};

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

        if diff.is_empty() {
            None
        } else {
            Some(diff)
        }
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

        if diff.is_empty() {
            None
        } else {
            Some(diff)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VecDiffValue<ValueDiff> {
    Create,
    Delete,
    Update(ValueDiff),
}

pub type VecDiff<V> = BTreeMap<String, VecDiffValue<<V as Diffable>::DiffResult>>;

impl<V> Diffable for Vec<V>
where
    V: Diffable,
    V::DiffResult: Serialize,
{
    type DiffResult = VecDiff<V>;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let mut diff = BTreeMap::new();

        let max_len = std::cmp::max(new.len(), current.len());

        for i in 0..max_len {
            match (new.get(i), current.get(i)) {
                (Some(n), Some(c)) => {
                    if let Some(d) = n.diff_with_current(c) {
                        diff.insert(i.to_string(), VecDiffValue::Update(d));
                    }
                }
                (Some(_), None) => {
                    diff.insert(i.to_string(), VecDiffValue::Create);
                }
                (None, Some(_)) => {
                    diff.insert(i.to_string(), VecDiffValue::Delete);
                }
                (None, None) => {}
            }
        }

        if diff.is_empty() {
            None
        } else {
            Some(diff)
        }
    }
}
