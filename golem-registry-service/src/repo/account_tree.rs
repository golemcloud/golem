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

//! Storage of the account tree: one current row per node in `account_nodes` and immutable history
//! in `account_node_revisions`.
//!
//! Record invariants:
//!
//! - Only the root has no parent. The database enforces one root, the parent foreign key and the
//!   unique `(parent_id, segment)` sibling key. A node's parent and depth never change.
//! - A node in deletion grace keeps its row, so its sibling key stays reserved until purge.
//!   Equal segments under different parents do not conflict.
//! - Every write updates the current row, appends the matching history row and, for a change
//!   that a cache can hold, appends an `AccountChanged` journal event in one transaction. A
//!   failed step rolls back all of them.
//! - Writes are optimistic: a change names the revision it read and fails with
//!   [`AccountTreeRepoError::ConcurrentModification`] when the node has moved on.
//! - Path and ancestry reads are single statements, so each result comes from one database
//!   snapshot.
//!
//! The tables are a preview schema in `db/preview/hierarchical-accounts`, which the production
//! migration flow does not run yet. [`preview_schema`] exposes it for tests.

use crate::repo::model::account_tree::{
    AccountLocalState, AccountNodeChange, AccountNodeRecord, AccountNodeRevisionRecord,
    AccountTreeRepoError,
};
use crate::repo::registry_change::{
    DbRegistryChangeRepo, NewRegistryChangeEvent, RequiresNotificationSignal, RequiresSignalExt,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::model::account_path::{AccountPath, AccountSegment};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{RepoError, SqlDateTime};
use indoc::indoc;
use uuid::Uuid;

/// The preview schema scripts, in order. They run after the production migrations.
pub mod preview_schema {
    pub const POSTGRES: &[&str] = &[include_str!(
        "../../db/preview/hierarchical-accounts/postgres/001_account_tree.sql"
    )];
    pub const SQLITE: &[&str] = &[include_str!(
        "../../db/preview/hierarchical-accounts/sqlite/001_account_tree.sql"
    )];
}

#[async_trait]
pub trait AccountTreeRepo: Send + Sync {
    async fn create_root(
        &self,
        account_id: Uuid,
        display_name: Option<String>,
        actor: Uuid,
    ) -> Result<AccountNodeRecord, AccountTreeRepoError>;

    async fn create_child(
        &self,
        account_id: Uuid,
        parent_id: Uuid,
        segment: AccountSegment,
        display_name: Option<String>,
        actor: Uuid,
    ) -> Result<AccountNodeRecord, AccountTreeRepoError>;

    /// Renames a node, changes its display name or changes its local state. The root can change
    /// only its display name.
    async fn update(
        &self,
        account_id: Uuid,
        expected_revision: u64,
        change: AccountNodeChange,
        actor: Uuid,
    ) -> Result<RequiresNotificationSignal<AccountNodeRecord>, AccountTreeRepoError>;

    /// Removes the current row of a childless node in deletion grace and releases its sibling
    /// key. Its history stays.
    async fn purge(
        &self,
        account_id: Uuid,
        expected_revision: u64,
        actor: Uuid,
    ) -> Result<RequiresNotificationSignal<()>, AccountTreeRepoError>;

    async fn get(&self, account_id: Uuid) -> Result<Option<AccountNodeRecord>, RepoError>;

    /// The node and its ancestors, root first, or an empty list when the node does not exist.
    async fn get_ancestry(&self, account_id: Uuid) -> Result<Vec<AccountNodeRecord>, RepoError>;

    /// The nodes along a path, root first, or an empty list when no node has the path.
    async fn resolve_path(&self, path: &AccountPath) -> Result<Vec<AccountNodeRecord>, RepoError>;

    async fn list_children(&self, parent_id: Uuid) -> Result<Vec<AccountNodeRecord>, RepoError>;

    async fn get_history(
        &self,
        account_id: Uuid,
    ) -> Result<Vec<AccountNodeRevisionRecord>, RepoError>;
}

pub struct DbAccountTreeRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "account_tree";

const NODE_COLUMNS: &str = "account_id, parent_id, segment, depth, display_name, local_state, \
    current_revision, created_at, updated_at, modified_by";

impl<DBP: Pool> DbAccountTreeRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

/// One statement that walks the path from the root by sibling key, then returns the nodes of
/// the chain in depth order.
fn resolve_path_query(depth: usize) -> String {
    let mut leaf = String::from("SELECT n0.account_id FROM account_nodes n0");
    for level in 1..=depth {
        leaf.push_str(&format!(
            " JOIN account_nodes n{level} ON n{level}.parent_id = n{prev}.account_id AND n{level}.segment = ${level}",
            prev = level - 1
        ));
    }
    leaf.push_str(" WHERE n0.parent_id IS NULL");
    let leaf_column = format!("n{depth}.account_id");
    let leaf = leaf.replacen("n0.account_id", &leaf_column, 1);
    format!(
        "WITH RECURSIVE leaf(account_id) AS ({leaf}), \
         chain AS (SELECT n.* FROM account_nodes n JOIN leaf ON n.account_id = leaf.account_id \
                   UNION ALL \
                   SELECT p.* FROM account_nodes p JOIN chain c ON p.account_id = c.parent_id) \
         SELECT {NODE_COLUMNS} FROM chain ORDER BY depth"
    )
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbAccountTreeRepo<PostgresPool> {
    async fn insert_node_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        record: &AccountNodeRecord,
    ) -> Result<AccountNodeRecord, RepoError> {
        tx.fetch_one_as(
            sqlx::query_as(&format!(
                "INSERT INTO account_nodes ({NODE_COLUMNS}) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {NODE_COLUMNS}"
            ))
            .bind(record.account_id)
            .bind(record.parent_id)
            .bind(&record.segment)
            .bind(record.depth)
            .bind(&record.display_name)
            .bind(record.local_state)
            .bind(record.current_revision)
            .bind(&record.created_at)
            .bind(&record.updated_at)
            .bind(record.modified_by),
        )
        .await
    }

    async fn insert_revision_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        record: &AccountNodeRecord,
    ) -> Result<(), AccountTreeRepoError> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO account_node_revisions
                    (account_id, revision, parent_id, segment, display_name, local_state,
                     created_at, created_by)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#})
            .bind(record.account_id)
            .bind(record.current_revision)
            .bind(record.parent_id)
            .bind(&record.segment)
            .bind(&record.display_name)
            .bind(record.local_state)
            .bind(&record.updated_at)
            .bind(record.modified_by),
        )
        .await
        .map_err(|err| {
            if err.is_unique_violation() {
                AccountTreeRepoError::ConcurrentModification
            } else {
                err.into()
            }
        })?;
        Ok(())
    }

    async fn get_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        account_id: Uuid,
    ) -> Result<Option<AccountNodeRecord>, RepoError> {
        tx.fetch_optional_as(
            sqlx::query_as(&format!(
                "SELECT {NODE_COLUMNS} FROM account_nodes WHERE account_id = $1"
            ))
            .bind(account_id),
        )
        .await
    }
}

fn new_node(
    account_id: Uuid,
    parent_id: Option<Uuid>,
    segment: String,
    depth: i16,
    display_name: Option<String>,
    actor: Uuid,
) -> AccountNodeRecord {
    let now = SqlDateTime::now();
    AccountNodeRecord {
        account_id,
        parent_id,
        segment,
        depth,
        display_name,
        local_state: AccountLocalState::Active.into(),
        current_revision: 0,
        created_at: now.clone(),
        updated_at: now,
        modified_by: actor,
    }
}

fn expected_revision(expected_revision: u64) -> Result<i64, AccountTreeRepoError> {
    i64::try_from(expected_revision).map_err(|_| AccountTreeRepoError::ConcurrentModification)
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl AccountTreeRepo for DbAccountTreeRepo<PostgresPool> {
    async fn create_root(
        &self,
        account_id: Uuid,
        display_name: Option<String>,
        actor: Uuid,
    ) -> Result<AccountNodeRecord, AccountTreeRepoError> {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "create_root", |tx| {
                async move {
                    let record = new_node(account_id, None, String::new(), 0, display_name, actor);
                    let record = Self::insert_node_in_tx(tx, &record).await.map_err(|err| {
                        if err.is_unique_violation() {
                            AccountTreeRepoError::RootExists
                        } else {
                            err.into()
                        }
                    })?;
                    Self::insert_revision_in_tx(tx, &record).await?;
                    Ok(record)
                }
                .boxed()
            })
            .await
    }

    async fn create_child(
        &self,
        account_id: Uuid,
        parent_id: Uuid,
        segment: AccountSegment,
        display_name: Option<String>,
        actor: Uuid,
    ) -> Result<AccountNodeRecord, AccountTreeRepoError> {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "create_child", |tx| {
                async move {
                    let parent = Self::get_in_tx(tx, parent_id)
                        .await?
                        .ok_or(AccountTreeRepoError::ParentNotFound)?;
                    if usize::try_from(parent.depth).unwrap_or(usize::MAX) >= AccountPath::MAX_DEPTH
                    {
                        return Err(AccountTreeRepoError::TooDeep);
                    }
                    let record = new_node(
                        account_id,
                        Some(parent_id),
                        segment.as_str().to_string(),
                        parent.depth + 1,
                        display_name,
                        actor,
                    );
                    // The parent foreign key also refuses a parent purged since the read above.
                    let record = Self::insert_node_in_tx(tx, &record).await.map_err(|err| {
                        if err.is_unique_violation() {
                            AccountTreeRepoError::SiblingConflict
                        } else if err.is_foreign_key_violation() {
                            AccountTreeRepoError::ParentNotFound
                        } else {
                            err.into()
                        }
                    })?;
                    Self::insert_revision_in_tx(tx, &record).await?;
                    Ok(record)
                }
                .boxed()
            })
            .await
    }

    async fn update(
        &self,
        account_id: Uuid,
        expected_revision: u64,
        change: AccountNodeChange,
        actor: Uuid,
    ) -> Result<RequiresNotificationSignal<AccountNodeRecord>, AccountTreeRepoError> {
        let expected = self::expected_revision(expected_revision)?;
        let record = self
            .db_pool
            .with_tx_err(METRICS_SVC_NAME, "update", |tx| {
                async move {
                    let current = Self::get_in_tx(tx, account_id)
                        .await?
                        .ok_or(AccountTreeRepoError::NotFound)?;
                    let segment = match (&current.parent_id, change.segment) {
                        (None, Some(_)) => return Err(AccountTreeRepoError::RootImmutable),
                        (None, None) if change.state != AccountLocalState::Active => {
                            return Err(AccountTreeRepoError::RootImmutable);
                        }
                        (_, Some(segment)) => segment.as_str().to_string(),
                        (_, None) => current.segment,
                    };
                    let record: AccountNodeRecord = tx
                        .fetch_optional_as(
                            sqlx::query_as(&format!(
                                "UPDATE account_nodes \
                                 SET segment = $1, display_name = $2, local_state = $3, \
                                     current_revision = current_revision + 1, updated_at = $4, \
                                     modified_by = $5 \
                                 WHERE account_id = $6 AND current_revision = $7 \
                                 RETURNING {NODE_COLUMNS}"
                            ))
                            .bind(&segment)
                            .bind(&change.display_name)
                            .bind(i16::from(change.state))
                            .bind(SqlDateTime::now())
                            .bind(actor)
                            .bind(account_id)
                            .bind(expected),
                        )
                        .await
                        .map_err(|err| {
                            if err.is_unique_violation() {
                                AccountTreeRepoError::SiblingConflict
                            } else {
                                err.into()
                            }
                        })?
                        .ok_or(AccountTreeRepoError::ConcurrentModification)?;
                    Self::insert_revision_in_tx(tx, &record).await?;
                    DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(
                        tx,
                        &NewRegistryChangeEvent::account_changed(account_id),
                    )
                    .await?;
                    Ok(record)
                }
                .boxed()
            })
            .await?;
        Ok(record.requires_notification_signal())
    }

    async fn purge(
        &self,
        account_id: Uuid,
        expected_revision: u64,
        _actor: Uuid,
    ) -> Result<RequiresNotificationSignal<()>, AccountTreeRepoError> {
        let expected = self::expected_revision(expected_revision)?;
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "purge", |tx| {
                async move {
                    let current = Self::get_in_tx(tx, account_id)
                        .await?
                        .ok_or(AccountTreeRepoError::NotFound)?;
                    if current.current_revision != expected {
                        return Err(AccountTreeRepoError::ConcurrentModification);
                    }
                    if current.local_state != i16::from(AccountLocalState::DeletionGrace) {
                        return Err(AccountTreeRepoError::NotInDeletionGrace);
                    }
                    let deleted = tx
                        .execute(
                            sqlx::query(
                                "DELETE FROM account_nodes \
                                 WHERE account_id = $1 AND current_revision = $2 AND local_state = $3",
                            )
                            .bind(account_id)
                            .bind(expected)
                            .bind(i16::from(AccountLocalState::DeletionGrace)),
                        )
                        .await
                        .map_err(|err| {
                            if err.is_foreign_key_violation() {
                                AccountTreeRepoError::HasChildren
                            } else {
                                err.into()
                            }
                        })?;
                    if deleted.rows_affected() != 1 {
                        return Err(AccountTreeRepoError::ConcurrentModification);
                    }
                    DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(
                        tx,
                        &NewRegistryChangeEvent::account_changed(account_id),
                    )
                    .await?;
                    Ok(())
                }
                .boxed()
            })
            .await?;
        Ok(().requires_notification_signal())
    }

    async fn get(&self, account_id: Uuid) -> Result<Option<AccountNodeRecord>, RepoError> {
        self.with_ro("get")
            .fetch_optional_as(
                sqlx::query_as(&format!(
                    "SELECT {NODE_COLUMNS} FROM account_nodes WHERE account_id = $1"
                ))
                .bind(account_id),
            )
            .await
    }

    async fn get_ancestry(&self, account_id: Uuid) -> Result<Vec<AccountNodeRecord>, RepoError> {
        self.with_ro("get_ancestry")
            .fetch_all_as(
                sqlx::query_as(&format!(
                    "WITH RECURSIVE chain AS ( \
                         SELECT n.* FROM account_nodes n WHERE n.account_id = $1 \
                         UNION ALL \
                         SELECT p.* FROM account_nodes p JOIN chain c ON p.account_id = c.parent_id) \
                     SELECT {NODE_COLUMNS} FROM chain ORDER BY depth"
                ))
                .bind(account_id),
            )
            .await
    }

    async fn resolve_path(&self, path: &AccountPath) -> Result<Vec<AccountNodeRecord>, RepoError> {
        let sql = resolve_path_query(path.depth());
        let mut query = sqlx::query_as(&sql);
        for segment in path.segments() {
            query = query.bind(segment.as_str());
        }
        self.with_ro("resolve_path").fetch_all_as(query).await
    }

    async fn list_children(&self, parent_id: Uuid) -> Result<Vec<AccountNodeRecord>, RepoError> {
        self.with_ro("list_children")
            .fetch_all_as(
                sqlx::query_as(&format!(
                    "SELECT {NODE_COLUMNS} FROM account_nodes WHERE parent_id = $1 ORDER BY segment"
                ))
                .bind(parent_id),
            )
            .await
    }

    async fn get_history(
        &self,
        account_id: Uuid,
    ) -> Result<Vec<AccountNodeRevisionRecord>, RepoError> {
        self.with_ro("get_history")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_id, revision, parent_id, segment, display_name, local_state,
                           created_at, created_by
                    FROM account_node_revisions
                    WHERE account_id = $1
                    ORDER BY revision
                "#})
                .bind(account_id),
            )
            .await
    }
}
