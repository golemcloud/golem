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
