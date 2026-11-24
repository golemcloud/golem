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

use golem_common::error_forwarding;
use golem_common::model::account_path::{AccountPath, AccountSegment};
use golem_service_base::repo::{RepoError, SqlDateTime};
use sqlx::FromRow;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AccountTreeRepoError {
    #[error("An account tree root already exists")]
    RootExists,
    #[error("The parent already has a child with this segment")]
    SiblingConflict,
    #[error("The parent account does not exist")]
    ParentNotFound,
    #[error("The parent account is at the maximum depth")]
    TooDeep,
    #[error("The account does not exist")]
    NotFound,
    #[error("The root account can change only its display name")]
    RootImmutable,
    #[error("Only an account in deletion grace can be purged")]
    NotInDeletionGrace,
    #[error("An account with children cannot be purged")]
    HasChildren,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(AccountTreeRepoError, RepoError);

/// A lifecycle state of an account, ordered from least to most restrictive.
///
/// Each account stores its *own state*: the state set on the account itself. A change to an
/// ancestor never overwrites it. The *effective state* of an account is the most restrictive own
/// state of the account and its ancestors (`DeletionGrace` > `Suspended` > `Active`); see
/// [`effective_state`]. For example, an `Active` child under a `Suspended` parent is effectively
/// `Suspended`, and lifting the parent's suspension does not clear a suspension that the child
/// has of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccountLifecycleState {
    Active,
    Suspended,
    /// Deleted and restorable. The node keeps its sibling key, so its name stays reserved
    /// until purge.
    DeletionGrace,
}

/// The effective state of the last node of a root-first ancestry: the most restrictive own state
/// along it.
pub fn effective_state<'a>(
    chain: impl IntoIterator<Item = &'a AccountNode>,
) -> AccountLifecycleState {
    chain
        .into_iter()
        .map(|node| node.own_state)
        .max()
        .unwrap_or(AccountLifecycleState::Active)
}

impl From<AccountLifecycleState> for i16 {
    fn from(value: AccountLifecycleState) -> Self {
        match value {
            AccountLifecycleState::Active => 0,
            AccountLifecycleState::Suspended => 1,
            AccountLifecycleState::DeletionGrace => 2,
        }
    }
}

impl TryFrom<i16> for AccountLifecycleState {
    type Error = anyhow::Error;

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AccountLifecycleState::Active),
            1 => Ok(AccountLifecycleState::Suspended),
            2 => Ok(AccountLifecycleState::DeletionGrace),
            other => Err(anyhow::anyhow!("Unknown account lifecycle state: {other}")),
        }
    }
}

/// The current row of an account tree node.
#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountNodeRecord {
    pub account_id: Uuid,
    pub parent_id: Option<Uuid>,
    /// Empty for the root.
    pub segment: String,
    pub depth: i16,
    pub display_name: Option<String>,
    pub own_state: i16,
    pub current_revision: i64,
    pub created_at: SqlDateTime,
    pub updated_at: SqlDateTime,
    pub modified_by: Uuid,
}

/// An immutable history row of an account tree node.
#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountNodeRevisionRecord {
    pub account_id: Uuid,
    pub revision: i64,
    pub parent_id: Option<Uuid>,
    pub segment: String,
    pub display_name: Option<String>,
    pub own_state: i16,
    pub created_at: SqlDateTime,
    pub created_by: Uuid,
}

/// The mutable fields of a non-root node. A same-parent rename needs no descendant check: the
/// segment grammar bounds every path, so a valid segment cannot make a descendant path too long.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountNodeChange {
    /// The new segment, or `None` to keep the current one. The root has no segment.
    pub segment: Option<AccountSegment>,
    pub display_name: Option<String>,
    /// The new own state of the node.
    pub own_state: AccountLifecycleState,
}

/// A validated account tree node.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountNode {
    pub account_id: Uuid,
    pub parent_id: Option<Uuid>,
    /// `None` only for the root.
    pub segment: Option<AccountSegment>,
    pub depth: usize,
    pub display_name: Option<String>,
    /// The state set on this account itself; see [`AccountLifecycleState`].
    pub own_state: AccountLifecycleState,
    pub revision: u64,
}

impl TryFrom<AccountNodeRecord> for AccountNode {
    type Error = anyhow::Error;

    fn try_from(record: AccountNodeRecord) -> Result<Self, Self::Error> {
        let segment = match record.parent_id {
            None => None,
            Some(_) => Some(
                AccountSegment::from_str(&record.segment)
                    .map_err(|err| anyhow::anyhow!("Stored account segment is invalid: {err}"))?,
            ),
        };
        Ok(Self {
            account_id: record.account_id,
            parent_id: record.parent_id,
            segment,
            depth: usize::try_from(record.depth)?,
            display_name: record.display_name,
            own_state: AccountLifecycleState::try_from(record.own_state)?,
            revision: u64::try_from(record.current_revision)?,
        })
    }
}

/// Converts a root-first ancestry to the account path of its last node. Returns an error when the
/// chain does not start at the root or a node is not the child of the one before it.
pub fn path_of_chain(chain: &[AccountNode]) -> anyhow::Result<AccountPath> {
    let mut path = AccountPath::root();
    let mut parent: Option<Uuid> = None;
    for (index, node) in chain.iter().enumerate() {
        if node.parent_id != parent || node.depth != index {
            return Err(anyhow::anyhow!(
                "Account {} is not a child of the previous node in its ancestry",
                node.account_id
            ));
        }
        if let Some(segment) = &node.segment {
            path = path.child(segment.clone())?;
        }
        parent = Some(node.account_id);
    }
    if chain.is_empty() {
        return Err(anyhow::anyhow!("An ancestry has at least the root"));
    }
    Ok(path)
}
