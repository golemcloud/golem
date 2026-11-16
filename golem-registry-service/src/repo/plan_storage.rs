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

//! Storage of plan definitions that accounts own for their direct children, their immutable
//! typed revisions, and the current and historical plan assignment of each account.
//!
//! Record invariants:
//!
//! - A definition has a stable ID, an immutable owner and a current revision. Each revision
//!   keeps its typed declarations, actor and time and never changes. Names are unique among an
//!   owner's selectable definitions.
//! - An assignment references the account together with its parent, and the definition together
//!   with its owner, so the database refuses a definition that the parent does not own. The root
//!   has no assignment; its baseline is installation state.
//! - Each assignment revision records the definition revision that was current at commit, so
//!   history keeps the policy an account had. An assignment follows later edits of its
//!   definition without a new assignment revision.
//! - A definition edit names the revision it read; an assignment names the assignment revision
//!   it read. Retries keep the caller's expected revision.
//!
//! Writes follow the guard protocol in [`crate::repo::account_guard`]. After acquiring its
//! guards, a write rereads its dependencies; a changed member set or ancestry rolls the attempt
//! back and rediscovers from the start, so a later attempt never adds a guard after an earlier
//! one. A write that changes effective policy checks every account below each affected root
//! and refuses a result in which any dimension has no valid value. It rejects only incompatible
//! ranges: overlapping and touching ranges are valid, and usage is not an input.
//!
//! An unused definition is not effective policy; it is checked when it is edited or assigned.

use crate::repo::account_guard::{DbGuards, GuardClass, GuardMode, GuardSet, is_guard_timeout};
use crate::repo::model::plan_storage::{
    PlanAssignmentRecord, PlanAssignmentRevisionRecord, PlanBoundRecord, PlanDefinition,
    PlanDefinitionRecord, PlanPolicy, PlanStorageRepoError, policy_from_bounds,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::model::plan_dimension::{
    DeclaredBounds, DimensionBoundsError, EffectiveBounds, InstallationAvailability, PlanDimension,
    resolve_effective_bounds,
};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::{NumericU64, RepoError, SqlDateTime};
use indoc::indoc;
use sqlx::FromRow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;
use uuid::Uuid;

pub const DEFAULT_GUARD_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_MAX_ATTEMPTS: usize = 8;

#[async_trait]
pub trait PlanStorageRepo: Send + Sync {
    async fn create_definition(
        &self,
        owner_account_id: Uuid,
        plan_id: Uuid,
        name: String,
        policy: PlanPolicy,
        actor: Uuid,
    ) -> Result<PlanDefinition, PlanStorageRepoError>;

    /// Creates a runtime-managed definition unless one with this ID exists. Returns whether it
    /// created the definition; an existing definition keeps its later edits.
    async fn seed_definition(
        &self,
        owner_account_id: Uuid,
        plan_id: Uuid,
        name: String,
        policy: PlanPolicy,
        actor: Uuid,
    ) -> Result<bool, PlanStorageRepoError>;

    async fn edit_definition(
        &self,
        plan_id: Uuid,
        expected_revision: u64,
        name: String,
        policy: PlanPolicy,
        actor: Uuid,
    ) -> Result<PlanDefinition, PlanStorageRepoError>;

    /// Removes an unassigned definition from selection. It keeps its ID and history.
    async fn retire_definition(
        &self,
        plan_id: Uuid,
        expected_revision: u64,
        actor: Uuid,
    ) -> Result<(), PlanStorageRepoError>;

    /// Assigns the account a definition of its parent. `expected_revision` is the assignment
    /// revision the caller read, or `None` when the account had no assignment.
    async fn assign(
        &self,
        account_id: Uuid,
        expected_revision: Option<u64>,
        plan_id: Uuid,
        actor: Uuid,
    ) -> Result<PlanAssignmentRecord, PlanStorageRepoError>;

    async fn get_definition(&self, plan_id: Uuid) -> Result<Option<PlanDefinition>, RepoError>;

    async fn get_definition_revision(
        &self,
        plan_id: Uuid,
        revision: u64,
    ) -> Result<Option<PlanPolicy>, RepoError>;

    async fn list_members(&self, plan_id: Uuid) -> Result<Vec<Uuid>, RepoError>;

    async fn get_assignment(
        &self,
        account_id: Uuid,
    ) -> Result<Option<PlanAssignmentRecord>, RepoError>;

    async fn get_assignment_history(
        &self,
        account_id: Uuid,
    ) -> Result<Vec<PlanAssignmentRevisionRecord>, RepoError>;

    /// The declared bounds of the account and its ancestors, combined per dimension, read in
    /// one statement. `None` when the account does not exist.
    async fn effective_bounds(
        &self,
        account_id: Uuid,
        availability: InstallationAvailability,
    ) -> Result<
        Option<BTreeMap<PlanDimension, Result<EffectiveBounds, DimensionBoundsError>>>,
        RepoError,
    >;
}

pub struct DbPlanStorageRepo<DBP: Pool> {
    db_pool: DBP,
    guard_timeout: Duration,
    max_attempts: usize,
}

static METRICS_SVC_NAME: &str = "plan_storage";

const DEFINITION_COLUMNS: &str = "plan_id, owner_account_id, name, selectable, current_revision, created_at, updated_at, modified_by";

const ASSIGNMENT_COLUMNS: &str =
    "account_id, parent_id, plan_id, revision, updated_at, modified_by";

const BOUND_COLUMNS: &str = "dimension, min_value, max_value, max_unlimited, default_value, \
    default_unlimited, user_configurable";

impl<DBP: Pool> DbPlanStorageRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self::with_limits(db_pool, DEFAULT_GUARD_TIMEOUT, DEFAULT_MAX_ATTEMPTS)
    }

    /// `guard_timeout` bounds each wait for a guard; `max_attempts` bounds the retries after a
    /// guard timeout or a changed member set.
    pub fn with_limits(db_pool: DBP, guard_timeout: Duration, max_attempts: usize) -> Self {
        Self {
            db_pool,
            guard_timeout,
            max_attempts: max_attempts.max(1),
        }
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

/// The outcome of one attempt of a guarded write.
#[derive(Debug)]
enum AttemptError {
    /// A guard wait timed out, or a dependency changed after discovery. The next attempt
    /// rediscovers from the start.
    Retry,
    Fail(PlanStorageRepoError),
}

impl From<RepoError> for AttemptError {
    fn from(error: RepoError) -> Self {
        if is_guard_timeout(&error) {
            AttemptError::Retry
        } else {
            AttemptError::Fail(error.into())
        }
    }
}

impl From<PlanStorageRepoError> for AttemptError {
    fn from(error: PlanStorageRepoError) -> Self {
        AttemptError::Fail(error)
    }
}

async fn backoff(attempt: usize) {
    tokio::time::sleep(Duration::from_millis(5 * attempt as u64)).await;
}

fn validate_policy(policy: &PlanPolicy) -> Result<(), PlanStorageRepoError> {
    for (dimension, declaration) in policy {
        match resolve_effective_bounds(
            *dimension,
            InstallationAvailability::Available,
            &[Some(declaration.bounds)],
        ) {
            Ok(_) | Err(DimensionBoundsError::MissingMaximum) => {}
            Err(error) => {
                return Err(PlanStorageRepoError::InvalidDeclaration {
                    dimension: *dimension,
                    error,
                });
            }
        }
    }
    Ok(())
}

#[derive(FromRow)]
struct TreeRow {
    account_id: Uuid,
    parent_id: Option<Uuid>,
    plan_id: Option<Uuid>,
}

#[derive(FromRow)]
struct EffectiveRow {
    depth: i16,
    dimension: Option<i16>,
    min_value: Option<NumericU64>,
    max_value: Option<NumericU64>,
    max_unlimited: Option<bool>,
    default_value: Option<NumericU64>,
    default_unlimited: Option<bool>,
    user_configurable: Option<bool>,
}

/// Checks every account of a protected subtree against its ancestors' declarations. `ancestors`
/// are the subtree root's ancestors, root first; `subtree` contains the root and all of its
/// descendants with the plan each one is assigned after the write.
fn check_subtree(
    ancestors: &[TreeRow],
    subtree: &[TreeRow],
    subtree_root: Uuid,
    policies: &HashMap<Uuid, PlanPolicy>,
) -> Result<(), PlanStorageRepoError> {
    let policy_of = |plan_id: Option<Uuid>| plan_id.and_then(|plan_id| policies.get(&plan_id));
    let prefix: Vec<Option<&PlanPolicy>> =
        ancestors.iter().map(|row| policy_of(row.plan_id)).collect();

    let mut children: HashMap<Uuid, Vec<&TreeRow>> = HashMap::new();
    let mut start = None;
    for row in subtree {
        if row.account_id == subtree_root {
            start = Some(row);
        } else if let Some(parent_id) = row.parent_id {
            children.entry(parent_id).or_default().push(row);
        }
    }
    let Some(start) = start else {
        return Ok(());
    };

    let mut stack = vec![(start, prefix)];
    while let Some((row, mut chain)) = stack.pop() {
        chain.push(policy_of(row.plan_id));
        for dimension in PlanDimension::ALL {
            let levels: Vec<Option<DeclaredBounds>> = chain
                .iter()
                .map(|policy| {
                    policy.and_then(|policy| policy.get(&dimension).map(|decl| decl.bounds))
                })
                .collect();
            if let Err(error @ DimensionBoundsError::Incompatible { .. }) =
                resolve_effective_bounds(dimension, InstallationAvailability::Available, &levels)
            {
                return Err(PlanStorageRepoError::Incompatible {
                    account_id: row.account_id,
                    dimension,
                    error,
                });
            }
        }
        for child in children.get(&row.account_id).into_iter().flatten() {
            stack.push((child, chain.clone()));
        }
    }
    Ok(())
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbPlanStorageRepo<PostgresPool> {
    async fn ancestry_ids_in(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        account_id: Uuid,
    ) -> Result<Vec<(Uuid, Option<Uuid>)>, RepoError> {
        tx.fetch_all_as(
            sqlx::query_as(indoc! { r#"
                WITH RECURSIVE chain AS (
                    SELECT account_id, parent_id, depth FROM account_nodes WHERE account_id = $1
                    UNION ALL
                    SELECT p.account_id, p.parent_id, p.depth
                    FROM account_nodes p JOIN chain c ON p.account_id = c.parent_id)
                SELECT account_id, parent_id FROM chain ORDER BY depth
            "#})
            .bind(account_id),
        )
        .await
    }

    async fn members_in(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        plan_id: Uuid,
    ) -> Result<Vec<Uuid>, RepoError> {
        let rows: Vec<(Uuid,)> = tx
            .fetch_all_as(
                sqlx::query_as(
                    "SELECT account_id FROM account_plan_assignments WHERE plan_id = $1 ORDER BY account_id",
                )
                .bind(plan_id),
            )
            .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn definition_in(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        plan_id: Uuid,
    ) -> Result<Option<PlanDefinitionRecord>, RepoError> {
        tx.fetch_optional_as(
            sqlx::query_as(&format!(
                "SELECT {DEFINITION_COLUMNS} FROM account_plans WHERE plan_id = $1"
            ))
            .bind(plan_id),
        )
        .await
    }

    async fn policy_in(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        plan_id: Uuid,
        revision: i64,
    ) -> Result<PlanPolicy, RepoError> {
        let records: Vec<PlanBoundRecord> = tx
            .fetch_all_as(
                sqlx::query_as(&format!(
                    "SELECT {BOUND_COLUMNS} FROM account_plan_revision_bounds \
                     WHERE plan_id = $1 AND revision = $2 ORDER BY dimension"
                ))
                .bind(plan_id)
                .bind(revision),
            )
            .await?;
        Ok(policy_from_bounds(&records)?)
    }

    async fn assignment_in(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        account_id: Uuid,
    ) -> Result<Option<PlanAssignmentRecord>, RepoError> {
        tx.fetch_optional_as(
            sqlx::query_as(&format!(
                "SELECT {ASSIGNMENT_COLUMNS} FROM account_plan_assignments WHERE account_id = $1"
            ))
            .bind(account_id),
        )
        .await
    }

    async fn insert_revision_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        record: &PlanDefinitionRecord,
        policy: &PlanPolicy,
    ) -> Result<(), RepoError> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO account_plan_revisions
                    (plan_id, revision, owner_account_id, name, created_at, created_by)
                VALUES ($1, $2, $3, $4, $5, $6)
            "#})
            .bind(record.plan_id)
            .bind(record.current_revision)
            .bind(record.owner_account_id)
            .bind(&record.name)
            .bind(&record.updated_at)
            .bind(record.modified_by),
        )
        .await?;
        for (dimension, declaration) in policy {
            let bound = PlanBoundRecord::from_declaration(*dimension, declaration);
            tx.execute(
                sqlx::query(&format!(
                    "INSERT INTO account_plan_revision_bounds (plan_id, revision, {BOUND_COLUMNS}) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
                ))
                .bind(record.plan_id)
                .bind(record.current_revision)
                .bind(bound.dimension)
                .bind(bound.min_value)
                .bind(bound.max_value)
                .bind(bound.max_unlimited)
                .bind(bound.default_value)
                .bind(bound.default_unlimited)
                .bind(bound.user_configurable),
            )
            .await?;
        }
        Ok(())
    }

    /// Checks the subtree under `subtree_root` with the given assignment and definition
    /// replacements applied. Must run after the guards that protect the subtree.
    async fn check_subtree_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        subtree_root: Uuid,
        assignment: Option<(Uuid, Uuid)>,
        definition: Option<(Uuid, &PlanPolicy)>,
    ) -> Result<(), AttemptError> {
        let mut ancestors: Vec<TreeRow> = tx
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    WITH RECURSIVE chain AS (
                        SELECT account_id, parent_id, depth FROM account_nodes WHERE account_id = $1
                        UNION ALL
                        SELECT p.account_id, p.parent_id, p.depth
                        FROM account_nodes p JOIN chain c ON p.account_id = c.parent_id)
                    SELECT c.account_id, c.parent_id, a.plan_id
                    FROM chain c LEFT JOIN account_plan_assignments a ON a.account_id = c.account_id
                    ORDER BY c.depth
                "#})
                .bind(subtree_root),
            )
            .await?;
        ancestors.pop();
        let mut subtree: Vec<TreeRow> = tx
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    WITH RECURSIVE sub AS (
                        SELECT account_id, parent_id FROM account_nodes WHERE account_id = $1
                        UNION ALL
                        SELECT n.account_id, n.parent_id
                        FROM account_nodes n JOIN sub s ON n.parent_id = s.account_id)
                    SELECT s.account_id, s.parent_id, a.plan_id
                    FROM sub s LEFT JOIN account_plan_assignments a ON a.account_id = s.account_id
                "#})
                .bind(subtree_root),
            )
            .await?;
        if let Some((account_id, plan_id)) = assignment {
            for row in subtree
                .iter_mut()
                .filter(|row| row.account_id == account_id)
            {
                row.plan_id = Some(plan_id);
            }
        }

        let plan_ids: BTreeSet<Uuid> = ancestors
            .iter()
            .chain(subtree.iter())
            .filter_map(|row| row.plan_id)
            .collect();
        let mut policies = HashMap::new();
        for plan_id in plan_ids {
            let policy = match definition {
                Some((edited, policy)) if edited == plan_id => policy.clone(),
                _ => {
                    let record = Self::definition_in(tx, plan_id).await?.ok_or_else(|| {
                        RepoError::InternalError(anyhow::anyhow!(
                            "Assigned plan {plan_id} has no definition"
                        ))
                    })?;
                    Self::policy_in(tx, plan_id, record.current_revision).await?
                }
            };
            policies.insert(plan_id, policy);
        }
        check_subtree(&ancestors, &subtree, subtree_root, &policies)?;
        Ok(())
    }

    async fn create_attempt(
        &self,
        owner_account_id: Uuid,
        plan_id: Uuid,
        name: &str,
        policy: &PlanPolicy,
        actor: Uuid,
        seed: bool,
    ) -> Result<Option<PlanDefinition>, AttemptError> {
        let mut ro = self.with_ro("create_definition_discover").begin().await?;
        let ancestry: Vec<Uuid> = Self::ancestry_ids_in(&mut ro, owner_account_id)
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        ro.commit().await?;
        if ancestry.is_empty() {
            return Err(PlanStorageRepoError::AccountNotFound.into());
        }
        let mut guards = GuardSet::new();
        guards
            .add_all(
                GuardClass::Account,
                ancestry.iter().copied(),
                GuardMode::Shared,
            )
            .add(GuardClass::Plan, plan_id, GuardMode::Exclusive);
        let timeout = self.guard_timeout;
        let name = name.to_string();
        let policy = policy.clone();
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "create_definition", |tx| {
                async move {
                    DbGuards::<PostgresPool>::acquire_in_tx(tx, &guards, timeout).await?;
                    let reread: Vec<Uuid> = Self::ancestry_ids_in(tx, owner_account_id)
                        .await?
                        .into_iter()
                        .map(|(id, _)| id)
                        .collect();
                    if reread != ancestry {
                        return Err(AttemptError::Retry);
                    }
                    if seed && Self::definition_in(tx, plan_id).await?.is_some() {
                        return Ok(None);
                    }
                    let now = SqlDateTime::now();
                    let record: PlanDefinitionRecord = tx
                        .fetch_one_as(
                            sqlx::query_as(&format!(
                                "INSERT INTO account_plans ({DEFINITION_COLUMNS}) \
                                 VALUES ($1, $2, $3, $4, 0, $5, $5, $6) RETURNING {DEFINITION_COLUMNS}"
                            ))
                            .bind(plan_id)
                            .bind(owner_account_id)
                            .bind(&name)
                            .bind(true)
                            .bind(now)
                            .bind(actor),
                        )
                        .await
                        .map_err(|err| {
                            if err.is_unique_violation() {
                                AttemptError::Fail(PlanStorageRepoError::NameConflict)
                            } else {
                                err.into()
                            }
                        })?;
                    Self::insert_revision_in_tx(tx, &record, &policy).await?;
                    Ok(Some(PlanDefinition { record, policy }))
                }
                .boxed()
            })
            .await
    }

    async fn edit_attempt(
        &self,
        plan_id: Uuid,
        expected_revision: i64,
        name: &str,
        policy: &PlanPolicy,
        actor: Uuid,
    ) -> Result<PlanDefinition, AttemptError> {
        let mut ro = self.with_ro("edit_definition_discover").begin().await?;
        let definition = Self::definition_in(&mut ro, plan_id)
            .await?
            .ok_or(PlanStorageRepoError::PlanNotFound)?;
        let members = Self::members_in(&mut ro, plan_id).await?;
        let owner_ancestry: Vec<Uuid> = Self::ancestry_ids_in(&mut ro, definition.owner_account_id)
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        ro.commit().await?;

        let mut guards = GuardSet::new();
        guards
            .add_all(
                GuardClass::Account,
                owner_ancestry.iter().copied(),
                GuardMode::Shared,
            )
            .add_all(
                GuardClass::Account,
                members.iter().copied(),
                GuardMode::Exclusive,
            )
            .add(GuardClass::Plan, plan_id, GuardMode::Exclusive);
        let timeout = self.guard_timeout;
        let name = name.to_string();
        let policy = policy.clone();
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "edit_definition", |tx| {
                async move {
                    DbGuards::<PostgresPool>::acquire_in_tx(tx, &guards, timeout).await?;
                    let current = Self::definition_in(tx, plan_id)
                        .await?
                        .ok_or(PlanStorageRepoError::PlanNotFound)?;
                    if current.current_revision != expected_revision {
                        return Err(PlanStorageRepoError::ConcurrentModification.into());
                    }
                    if !current.selectable {
                        return Err(PlanStorageRepoError::NotSelectable.into());
                    }
                    if Self::members_in(tx, plan_id).await? != members {
                        return Err(AttemptError::Retry);
                    }
                    for member in &members {
                        Self::check_subtree_in_tx(tx, *member, None, Some((plan_id, &policy)))
                            .await?;
                    }
                    let record: PlanDefinitionRecord = tx
                        .fetch_optional_as(
                            sqlx::query_as(&format!(
                                "UPDATE account_plans SET name = $1, current_revision = $2, \
                                 updated_at = $3, modified_by = $4 \
                                 WHERE plan_id = $5 AND current_revision = $6 \
                                 RETURNING {DEFINITION_COLUMNS}"
                            ))
                            .bind(&name)
                            .bind(expected_revision + 1)
                            .bind(SqlDateTime::now())
                            .bind(actor)
                            .bind(plan_id)
                            .bind(expected_revision),
                        )
                        .await
                        .map_err(|err| {
                            if err.is_unique_violation() {
                                AttemptError::Fail(PlanStorageRepoError::NameConflict)
                            } else {
                                err.into()
                            }
                        })?
                        .ok_or(PlanStorageRepoError::ConcurrentModification)?;
                    Self::insert_revision_in_tx(tx, &record, &policy).await?;
                    Ok(PlanDefinition { record, policy })
                }
                .boxed()
            })
            .await
    }

    async fn retire_attempt(
        &self,
        plan_id: Uuid,
        expected_revision: i64,
        actor: Uuid,
    ) -> Result<(), AttemptError> {
        let mut ro = self.with_ro("retire_definition_discover").begin().await?;
        let definition = Self::definition_in(&mut ro, plan_id)
            .await?
            .ok_or(PlanStorageRepoError::PlanNotFound)?;
        let owner_ancestry: Vec<Uuid> = Self::ancestry_ids_in(&mut ro, definition.owner_account_id)
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        ro.commit().await?;

        let mut guards = GuardSet::new();
        guards
            .add_all(GuardClass::Account, owner_ancestry, GuardMode::Shared)
            .add(GuardClass::Plan, plan_id, GuardMode::Exclusive);
        let timeout = self.guard_timeout;
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "retire_definition", |tx| {
                async move {
                    DbGuards::<PostgresPool>::acquire_in_tx(tx, &guards, timeout).await?;
                    let current = Self::definition_in(tx, plan_id)
                        .await?
                        .ok_or(PlanStorageRepoError::PlanNotFound)?;
                    if current.current_revision != expected_revision {
                        return Err(PlanStorageRepoError::ConcurrentModification.into());
                    }
                    if !Self::members_in(tx, plan_id).await?.is_empty() {
                        return Err(PlanStorageRepoError::Assigned.into());
                    }
                    tx.execute(
                        sqlx::query(
                            "UPDATE account_plans SET selectable = $1, updated_at = $2, modified_by = $3 \
                             WHERE plan_id = $4 AND current_revision = $5",
                        )
                        .bind(false)
                        .bind(SqlDateTime::now())
                        .bind(actor)
                        .bind(plan_id)
                        .bind(expected_revision),
                    )
                    .await?;
                    Ok(())
                }
                .boxed()
            })
            .await
    }

    async fn assign_attempt(
        &self,
        account_id: Uuid,
        expected_revision: Option<i64>,
        plan_id: Uuid,
        actor: Uuid,
    ) -> Result<PlanAssignmentRecord, AttemptError> {
        let mut ro = self.with_ro("assign_discover").begin().await?;
        let ancestry = Self::ancestry_ids_in(&mut ro, account_id).await?;
        let old_plan = Self::assignment_in(&mut ro, account_id)
            .await?
            .map(|assignment| assignment.plan_id);
        ro.commit().await?;
        let Some((_, parent_id)) = ancestry.last().copied() else {
            return Err(PlanStorageRepoError::AccountNotFound.into());
        };
        let Some(parent_id) = parent_id else {
            return Err(PlanStorageRepoError::RootAssignment.into());
        };

        let mut guards = GuardSet::new();
        guards
            .add_all(
                GuardClass::Account,
                ancestry[..ancestry.len() - 1].iter().map(|(id, _)| *id),
                GuardMode::Shared,
            )
            .add(GuardClass::Account, account_id, GuardMode::Exclusive)
            .add(GuardClass::Plan, plan_id, GuardMode::Shared);
        if let Some(old_plan) = old_plan {
            guards.add(GuardClass::Plan, old_plan, GuardMode::Shared);
        }
        let timeout = self.guard_timeout;
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "assign", |tx| {
                async move {
                    DbGuards::<PostgresPool>::acquire_in_tx(tx, &guards, timeout).await?;
                    if Self::ancestry_ids_in(tx, account_id).await? != ancestry {
                        return Err(AttemptError::Retry);
                    }
                    let current = Self::assignment_in(tx, account_id).await?;
                    match (&current, expected_revision) {
                        (None, None) => {}
                        (Some(current), Some(expected)) if current.revision == expected => {}
                        _ => return Err(PlanStorageRepoError::ConcurrentModification.into()),
                    }
                    if current.as_ref().map(|current| current.plan_id) != old_plan {
                        return Err(AttemptError::Retry);
                    }
                    let destination = Self::definition_in(tx, plan_id)
                        .await?
                        .ok_or(PlanStorageRepoError::PlanNotFound)?;
                    if destination.owner_account_id != parent_id {
                        return Err(PlanStorageRepoError::ForeignCatalog.into());
                    }
                    if !destination.selectable {
                        return Err(PlanStorageRepoError::NotSelectable.into());
                    }
                    Self::check_subtree_in_tx(tx, account_id, Some((account_id, plan_id)), None)
                        .await?;

                    let now = SqlDateTime::now();
                    let record: PlanAssignmentRecord = match expected_revision {
                        None => {
                            tx.fetch_one_as(
                                sqlx::query_as(&format!(
                                    "INSERT INTO account_plan_assignments ({ASSIGNMENT_COLUMNS}) \
                                     VALUES ($1, $2, $3, 0, $4, $5) RETURNING {ASSIGNMENT_COLUMNS}"
                                ))
                                .bind(account_id)
                                .bind(parent_id)
                                .bind(plan_id)
                                .bind(now.clone())
                                .bind(actor),
                            )
                            .await
                            .map_err(|err| {
                                if err.is_unique_violation() {
                                    AttemptError::Fail(PlanStorageRepoError::ConcurrentModification)
                                } else {
                                    err.into()
                                }
                            })?
                        }
                        Some(expected) => tx
                            .fetch_optional_as(
                                sqlx::query_as(&format!(
                                    "UPDATE account_plan_assignments \
                                     SET plan_id = $1, revision = $2, updated_at = $3, modified_by = $4 \
                                     WHERE account_id = $5 AND revision = $6 \
                                     RETURNING {ASSIGNMENT_COLUMNS}"
                                ))
                                .bind(plan_id)
                                .bind(expected + 1)
                                .bind(now.clone())
                                .bind(actor)
                                .bind(account_id)
                                .bind(expected),
                            )
                            .await?
                            .ok_or(PlanStorageRepoError::ConcurrentModification)?,
                    };
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            INSERT INTO account_plan_assignment_revisions
                                (account_id, revision, plan_id, plan_revision, created_at, created_by)
                            VALUES ($1, $2, $3, $4, $5, $6)
                        "#})
                        .bind(account_id)
                        .bind(record.revision)
                        .bind(plan_id)
                        .bind(destination.current_revision)
                        .bind(now)
                        .bind(actor),
                    )
                    .await?;
                    Ok(record)
                }
                .boxed()
            })
            .await
    }
}

fn expected(revision: u64) -> Result<i64, PlanStorageRepoError> {
    i64::try_from(revision).map_err(|_| PlanStorageRepoError::ConcurrentModification)
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl PlanStorageRepo for DbPlanStorageRepo<PostgresPool> {
    async fn create_definition(
        &self,
        owner_account_id: Uuid,
        plan_id: Uuid,
        name: String,
        policy: PlanPolicy,
        actor: Uuid,
    ) -> Result<PlanDefinition, PlanStorageRepoError> {
        validate_policy(&policy)?;
        for attempt in 1..=self.max_attempts {
            match self
                .create_attempt(owner_account_id, plan_id, &name, &policy, actor, false)
                .await
            {
                Ok(Some(definition)) => return Ok(definition),
                Ok(None) => return Err(PlanStorageRepoError::ConcurrentModification),
                Err(AttemptError::Retry) => backoff(attempt).await,
                Err(AttemptError::Fail(error)) => return Err(error),
            }
        }
        Err(PlanStorageRepoError::Contention)
    }

    async fn seed_definition(
        &self,
        owner_account_id: Uuid,
        plan_id: Uuid,
        name: String,
        policy: PlanPolicy,
        actor: Uuid,
    ) -> Result<bool, PlanStorageRepoError> {
        validate_policy(&policy)?;
        for attempt in 1..=self.max_attempts {
            match self
                .create_attempt(owner_account_id, plan_id, &name, &policy, actor, true)
                .await
            {
                Ok(created) => return Ok(created.is_some()),
                Err(AttemptError::Retry) => backoff(attempt).await,
                Err(AttemptError::Fail(error)) => return Err(error),
            }
        }
        Err(PlanStorageRepoError::Contention)
    }

    async fn edit_definition(
        &self,
        plan_id: Uuid,
        expected_revision: u64,
        name: String,
        policy: PlanPolicy,
        actor: Uuid,
    ) -> Result<PlanDefinition, PlanStorageRepoError> {
        validate_policy(&policy)?;
        let expected_revision = expected(expected_revision)?;
        for attempt in 1..=self.max_attempts {
            match self
                .edit_attempt(plan_id, expected_revision, &name, &policy, actor)
                .await
            {
                Ok(definition) => return Ok(definition),
                Err(AttemptError::Retry) => backoff(attempt).await,
                Err(AttemptError::Fail(error)) => return Err(error),
            }
        }
        Err(PlanStorageRepoError::Contention)
    }

    async fn retire_definition(
        &self,
        plan_id: Uuid,
        expected_revision: u64,
        actor: Uuid,
    ) -> Result<(), PlanStorageRepoError> {
        let expected_revision = expected(expected_revision)?;
        for attempt in 1..=self.max_attempts {
            match self.retire_attempt(plan_id, expected_revision, actor).await {
                Ok(()) => return Ok(()),
                Err(AttemptError::Retry) => backoff(attempt).await,
                Err(AttemptError::Fail(error)) => return Err(error),
            }
        }
        Err(PlanStorageRepoError::Contention)
    }

    async fn assign(
        &self,
        account_id: Uuid,
        expected_revision: Option<u64>,
        plan_id: Uuid,
        actor: Uuid,
    ) -> Result<PlanAssignmentRecord, PlanStorageRepoError> {
        let expected_revision = expected_revision.map(expected).transpose()?;
        for attempt in 1..=self.max_attempts {
            match self
                .assign_attempt(account_id, expected_revision, plan_id, actor)
                .await
            {
                Ok(record) => return Ok(record),
                Err(AttemptError::Retry) => backoff(attempt).await,
                Err(AttemptError::Fail(error)) => return Err(error),
            }
        }
        Err(PlanStorageRepoError::Contention)
    }

    async fn get_definition(&self, plan_id: Uuid) -> Result<Option<PlanDefinition>, RepoError> {
        let mut ro = self.with_ro("get_definition").begin().await?;
        let Some(record) = Self::definition_in(&mut ro, plan_id).await? else {
            return Ok(None);
        };
        let policy = Self::policy_in(&mut ro, plan_id, record.current_revision).await?;
        ro.commit().await?;
        Ok(Some(PlanDefinition { record, policy }))
    }

    async fn get_definition_revision(
        &self,
        plan_id: Uuid,
        revision: u64,
    ) -> Result<Option<PlanPolicy>, RepoError> {
        let Ok(revision) = i64::try_from(revision) else {
            return Ok(None);
        };
        let mut ro = self.with_ro("get_definition_revision").begin().await?;
        let exists: Option<(i64,)> = ro
            .fetch_optional_as(
                sqlx::query_as(
                    "SELECT revision FROM account_plan_revisions WHERE plan_id = $1 AND revision = $2",
                )
                .bind(plan_id)
                .bind(revision),
            )
            .await?;
        if exists.is_none() {
            return Ok(None);
        }
        let policy = Self::policy_in(&mut ro, plan_id, revision).await?;
        ro.commit().await?;
        Ok(Some(policy))
    }

    async fn list_members(&self, plan_id: Uuid) -> Result<Vec<Uuid>, RepoError> {
        let mut ro = self.with_ro("list_members").begin().await?;
        let members = Self::members_in(&mut ro, plan_id).await?;
        ro.commit().await?;
        Ok(members)
    }

    async fn get_assignment(
        &self,
        account_id: Uuid,
    ) -> Result<Option<PlanAssignmentRecord>, RepoError> {
        let mut ro = self.with_ro("get_assignment").begin().await?;
        let assignment = Self::assignment_in(&mut ro, account_id).await?;
        ro.commit().await?;
        Ok(assignment)
    }

    async fn get_assignment_history(
        &self,
        account_id: Uuid,
    ) -> Result<Vec<PlanAssignmentRevisionRecord>, RepoError> {
        self.with_ro("get_assignment_history")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_id, revision, plan_id, plan_revision, created_at, created_by
                    FROM account_plan_assignment_revisions
                    WHERE account_id = $1
                    ORDER BY revision
                "#})
                .bind(account_id),
            )
            .await
    }

    async fn effective_bounds(
        &self,
        account_id: Uuid,
        availability: InstallationAvailability,
    ) -> Result<
        Option<BTreeMap<PlanDimension, Result<EffectiveBounds, DimensionBoundsError>>>,
        RepoError,
    > {
        let rows: Vec<EffectiveRow> = self
            .with_ro("effective_bounds")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    WITH RECURSIVE chain AS (
                        SELECT account_id, parent_id, depth FROM account_nodes WHERE account_id = $1
                        UNION ALL
                        SELECT p.account_id, p.parent_id, p.depth
                        FROM account_nodes p JOIN chain c ON p.account_id = c.parent_id)
                    SELECT c.depth, b.dimension, b.min_value, b.max_value, b.max_unlimited,
                           b.default_value, b.default_unlimited, b.user_configurable
                    FROM chain c
                    LEFT JOIN account_plan_assignments a ON a.account_id = c.account_id
                    LEFT JOIN account_plans p ON p.plan_id = a.plan_id
                    LEFT JOIN account_plan_revision_bounds b
                        ON b.plan_id = p.plan_id AND b.revision = p.current_revision
                    ORDER BY c.depth
                "#})
                .bind(account_id),
            )
            .await?;
        let Some(depth) = rows.iter().map(|row| row.depth).max() else {
            return Ok(None);
        };
        let levels = usize::try_from(depth).map_err(anyhow::Error::from)? + 1;
        let mut chains: BTreeMap<PlanDimension, Vec<Option<DeclaredBounds>>> = PlanDimension::ALL
            .into_iter()
            .map(|dimension| (dimension, vec![None; levels]))
            .collect();
        for row in rows {
            let Some(code) = row.dimension else { continue };
            let record = PlanBoundRecord {
                dimension: code,
                min_value: row.min_value,
                max_value: row.max_value,
                max_unlimited: row.max_unlimited.unwrap_or(false),
                default_value: row.default_value,
                default_unlimited: row.default_unlimited.unwrap_or(false),
                user_configurable: row.user_configurable,
            };
            let (dimension, declaration) = record.to_declaration()?;
            let level = usize::try_from(row.depth).map_err(anyhow::Error::from)?;
            chains
                .get_mut(&dimension)
                .expect("every dimension has a chain")[level] = Some(declaration.bounds);
        }
        Ok(Some(
            chains
                .into_iter()
                .map(|(dimension, chain)| {
                    (
                        dimension,
                        resolve_effective_bounds(dimension, availability, &chain),
                    )
                })
                .collect(),
        ))
    }
}
