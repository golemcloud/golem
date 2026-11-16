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

//! The structured form of a replay divergence, and the class of a failed update.

use crate::base_model::OplogIndex;
use crate::base_model::oplog::{DurableInvocationId, PublicDurableFunctionType};
use crate::schema::TypedSchemaValue;
use crate::{declare_enums, declare_unions};
use golem_schema::Empty;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};

/// Where a replay left the recorded history: the first entry at which the replay did not
/// reproduce the recorded execution, what the oplog holds there and what the replay did instead.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ReplayDivergence {
    /// The oplog index of the first divergence.
    pub oplog_index: OplogIndex,
    /// What the oplog holds at the index.
    pub expected: ReplayStep,
    /// What the replay did at the index.
    pub actual: ReplayStep,
    /// The identity fields on which `expected` and `actual` differ; empty when the steps differ
    /// in kind.
    pub mismatched: Vec<StartIdentityField>,
}

impl ReplayDivergence {
    /// A divergence at `oplog_index` between two steps that only a text describes.
    pub fn described(
        oplog_index: OplogIndex,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self {
            oplog_index,
            expected: ReplayStep::described(expected),
            actual: ReplayStep::described(actual),
            mismatched: Vec::new(),
        }
    }
}

impl Display for ReplayDivergence {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Unexpected oplog entry at {}: expected {}, got {}",
            self.oplog_index, self.expected, self.actual
        )?;
        if !self.mismatched.is_empty() {
            let fields = self
                .mismatched
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            write!(f, " (mismatched: {fields})")?;
        }
        Ok(())
    }
}

declare_unions! {
    /// One step of a replay: a host call, the end of an invocation, the end of the recorded
    /// history, or another step that a text describes.
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    #[derive(Eq, Hash)]
    pub enum ReplayStep {
        HostCall(ReplayHostCall),
        InvocationEnd(Empty),
        EndOfHistory(Empty),
        Other(ReplayStepDescription),
    }
}

impl ReplayStep {
    pub fn described(description: impl Into<String>) -> Self {
        Self::Other(ReplayStepDescription {
            description: description.into(),
        })
    }

    pub fn end_of_history() -> Self {
        Self::EndOfHistory(Empty {})
    }

    pub fn invocation_end() -> Self {
        Self::InvocationEnd(Empty {})
    }
}

impl Display for ReplayStep {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostCall(call) => write!(f, "{call}"),
            Self::InvocationEnd(_) => write!(f, "the end of the invocation"),
            Self::EndOfHistory(_) => write!(f, "the end of the recorded history"),
            Self::Other(description) => write!(f, "{}", description.description),
        }
    }
}

/// A step of a replay that a text describes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ReplayStepDescription {
    pub description: String,
}

/// A host call of a replay: the identity of a recorded `Start`, or of the call that the replay
/// made in its place.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ReplayHostCall {
    pub function_name: String,
    pub durable_function_type: PublicDurableFunctionType,
    pub parent_start_index: Option<OplogIndex>,
    pub invocation_id: Option<DurableInvocationId>,
    pub observational_owner: Option<OplogIndex>,
    /// Whether the call carries a request: `true` for a durable call, `false` for a durable
    /// scope.
    pub carries_request: bool,
    /// The request of the call, when it is known.
    pub request: Option<Box<TypedSchemaValue>>,
}

impl ReplayHostCall {
    /// The identity fields on which `self` and `other` differ.
    pub fn mismatched_fields(&self, other: &ReplayHostCall) -> Vec<StartIdentityField> {
        [
            (
                self.function_name != other.function_name,
                StartIdentityField::FunctionName,
            ),
            (
                self.durable_function_type != other.durable_function_type,
                StartIdentityField::FunctionType,
            ),
            (
                self.invocation_id != other.invocation_id,
                StartIdentityField::InvocationId,
            ),
            (
                self.observational_owner != other.observational_owner,
                StartIdentityField::ObservationalOwner,
            ),
            (
                self.carries_request != other.carries_request,
                StartIdentityField::RequestPresence,
            ),
            (
                self.parent_start_index != other.parent_start_index,
                StartIdentityField::Parent,
            ),
        ]
        .into_iter()
        .filter_map(|(differs, field)| differs.then_some(field))
        .collect()
    }
}

// Two host calls are the same call when their identities and requests are equal. The requests
// compare by their schema graphs and by the bits of their float leaves, so a request that holds a
// NaN equals itself and the equality stays reflexive. Hashing leaves the request out: equal calls
// still have equal hashes.
impl PartialEq for ReplayHostCall {
    fn eq(&self, other: &Self) -> bool {
        self.function_name == other.function_name
            && self.durable_function_type == other.durable_function_type
            && self.parent_start_index == other.parent_start_index
            && self.invocation_id == other.invocation_id
            && self.observational_owner == other.observational_owner
            && self.carries_request == other.carries_request
            && match (&self.request, &other.request) {
                (None, None) => true,
                (Some(a), Some(b)) => a.graph() == b.graph() && a.value().bitwise_eq(b.value()),
                _ => false,
            }
    }
}

impl Eq for ReplayHostCall {}

impl Hash for ReplayHostCall {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.function_name.hash(state);
        self.durable_function_type.hash(state);
        self.parent_start_index.hash(state);
        self.invocation_id.hash(state);
        self.observational_owner.hash(state);
        self.carries_request.hash(state);
    }
}

impl Display for ReplayHostCall {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let function_type = match self.durable_function_type {
            PublicDurableFunctionType::ReadLocal(_) => "ReadLocal",
            PublicDurableFunctionType::WriteLocal(_) => "WriteLocal",
            PublicDurableFunctionType::ReadRemote(_) => "ReadRemote",
            PublicDurableFunctionType::WriteRemote(_) => "WriteRemote",
        };
        write!(
            f,
            "Start {{ {}, {function_type}, request: {}, parent_start_index: {:?} }}",
            self.function_name,
            if self.carries_request {
                "Some(..)"
            } else {
                "None"
            },
            self.parent_start_index.map(u64::from)
        )?;
        if let Some(invocation_id) = self.invocation_id {
            write!(f, " with invocation id {invocation_id}")?;
        }
        if let Some(owner) = self.observational_owner {
            write!(f, " observed by {owner}")?;
        }
        Ok(())
    }
}

declare_enums! {
    /// An identity field of a recorded `Start`, which a replayed host call must match.
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    #[derive(Hash)]
    pub enum StartIdentityField {
        FunctionName,
        FunctionType,
        InvocationId,
        ObservationalOwner,
        RequestPresence,
        Parent,
    }

    /// Why an update failed.
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    #[derive(Hash)]
    pub enum UpdateFailureClass {
        /// The history did not replay on the target: an unmatched entry, or a divergence after a
        /// snapshot.
        Divergence,
        /// The target code hit a Wasm instruction trap, panicked, or returned an agent error on
        /// recorded inputs.
        TargetTrap,
        /// Replaying needs a peer effect.
        NotReplayable,
        /// The target revision was missing, refused or unsupported, or changes the agent's mode.
        TargetRejected,
        /// The replay was interrupted.
        Interrupted,
        /// An operational failure: storage, load, quota, an infrastructure or non-deterministic
        /// error.
        Operational,
        /// Snapshot, restore and other failures.
        Other,
    }
}
