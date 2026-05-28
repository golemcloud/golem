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

use serde::{Deserialize, Serialize};
use std::fmt::Debug;

mod account;
mod agent;
mod application;
mod component;
mod empty;
mod environment;
mod tool;

pub use account::*;
pub use agent::*;
pub use application::*;
pub use component::*;
pub use empty::*;
pub use environment::*;
pub use tool::*;

#[cfg(feature = "full")]
pub trait OwnerPattern:
    Debug
    + Clone
    + PartialEq
    + Eq
    + Serialize
    + for<'de> Deserialize<'de>
    + desert_rust::BinarySerializer
    + desert_rust::BinaryDeserializer
{
    type Polymorphic: Debug
        + Clone
        + PartialEq
        + Eq
        + Serialize
        + for<'de> Deserialize<'de>
        + desert_rust::BinarySerializer
        + desert_rust::BinaryDeserializer;

    fn parse(value: &str) -> Result<Self, String>
    where
        Self: Sized;
    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String>
    where
        Self: Sized;

    fn subsumes(&self, other: &Self) -> bool;
}

#[cfg(not(feature = "full"))]
pub trait OwnerPattern:
    Debug + Clone + PartialEq + Eq + Serialize + for<'de> Deserialize<'de>
{
    type Polymorphic: Debug + Clone + PartialEq + Eq + Serialize + for<'de> Deserialize<'de>;

    fn parse(value: &str) -> Result<Self, String>
    where
        Self: Sized;
    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String>
    where
        Self: Sized;

    fn subsumes(&self, other: &Self) -> bool;
}

pub(super) enum PrefixOwnerSlot<T> {
    Concrete(T),
    Env,
    Self_,
}

pub(super) fn parse_prefix_owner_slot<T>(
    value: &str,
    parse_concrete: impl Fn(&str) -> Result<T, String>,
) -> Result<PrefixOwnerSlot<T>, String> {
    match split_leftmost_owner_slot(value)? {
        Some(("?env", rest)) if rest.is_empty() => Ok(PrefixOwnerSlot::Env),
        Some(("?self", rest)) if rest.is_empty() => Ok(PrefixOwnerSlot::Self_),
        Some(_) => Err(value.to_string()),
        None => parse_concrete(value).map(PrefixOwnerSlot::Concrete),
    }
}

pub(super) fn split_leftmost_owner_slot(value: &str) -> Result<Option<(&str, Vec<&str>)>, String> {
    if value.is_empty() {
        return Err(value.to_string());
    }

    let segments = parse_segments(value)?;
    let Some((first, rest)) = segments.split_first() else {
        return Err(value.to_string());
    };

    if first.starts_with('?') {
        if !matches!(*first, "?env" | "?self") || rest.iter().any(|segment| segment.contains('?')) {
            Err(value.to_string())
        } else {
            Ok(Some((*first, rest.to_vec())))
        }
    } else if contains_slot_reference(value) {
        Err(value.to_string())
    } else {
        Ok(None)
    }
}

pub(super) fn parse_segments(value: &str) -> Result<Vec<&str>, String> {
    if value.is_empty() {
        return Err(value.to_string());
    }

    let segments: Vec<&str> = value.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        Err(value.to_string())
    } else {
        Ok(segments)
    }
}

pub(super) fn parse_concrete_segment(value: &str) -> Result<&str, String> {
    if value.is_empty() || value == "*" || value.starts_with('?') {
        Err(value.to_string())
    } else {
        Ok(value)
    }
}

fn contains_slot_reference(value: &str) -> bool {
    value.split('/').any(|segment| segment.starts_with('?'))
}
