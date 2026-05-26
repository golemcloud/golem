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

use crate::base_model::card::CardBinaryCodec;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

mod account;
mod agent;
mod environment;

pub use account::*;
pub use agent::*;
pub use environment::*;

pub trait RecipientPattern:
    Debug + Clone + PartialEq + Eq + Serialize + for<'de> Deserialize<'de> + CardBinaryCodec + Sized
{
    type Polymorphic: Debug
        + Clone
        + PartialEq
        + Eq
        + Serialize
        + for<'de> Deserialize<'de>
        + CardBinaryCodec;

    fn parse(value: &str) -> Result<Self, String>;
    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String>;
    fn subsumes(&self, other: &Self) -> bool;
    fn matches_holder(&self, holder: &str) -> bool;
}

pub(super) fn parse_anchored_segments(value: &str) -> Result<Vec<&str>, String> {
    if value.is_empty() {
        return Err(value.to_string());
    }

    let segments = value.split('/').collect::<Vec<_>>();
    if segments.first() == Some(&"*")
        || segments.iter().any(|segment| segment.is_empty())
        || has_segment_after_wildcard_segment(&segments)
    {
        Err(value.to_string())
    } else {
        Ok(segments)
    }
}

pub(super) fn parse_holder_segments(value: &str) -> Result<Vec<&str>, String> {
    let segments = parse_anchored_segments(value)?;
    match segments.len() {
        1 | 3 | 5 => Ok(segments),
        _ => Err(value.to_string()),
    }
}

pub(super) fn has_segment_after_wildcard_segment(segments: &[&str]) -> bool {
    let mut seen_wildcard = false;
    for segment in segments {
        match *segment {
            "*" => seen_wildcard = true,
            _ if seen_wildcard => return true,
            _ => {}
        }
    }
    false
}

pub(super) fn split_leftmost_slot(value: &str) -> Result<Option<(&str, Vec<&str>)>, String> {
    let segments = value.split('/').collect::<Vec<_>>();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(value.to_string());
    }

    let Some((first, rest)) = segments.split_first() else {
        return Err(value.to_string());
    };

    if first.starts_with('?') {
        if rest.iter().any(|segment| segment.contains('?'))
            || has_segment_after_wildcard_segment(rest)
        {
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

pub(super) fn valid_suffix_segment(segment: &str) -> bool {
    !segment.is_empty() && segment != "*" && !segment.contains('?')
}

fn contains_slot_reference(value: &str) -> bool {
    value
        .split('/')
        .any(|segment| segment.starts_with('?') || segment.contains("/?"))
}

pub(super) fn agent_segment_subsumes(left: &str, right: &str) -> bool {
    if left == right || left == "*" {
        return true;
    }
    let Some(agent_type) = left.strip_suffix("(*)") else {
        return false;
    };
    right
        .strip_prefix(agent_type)
        .is_some_and(|suffix| suffix.starts_with('(') && suffix.ends_with(')'))
}
