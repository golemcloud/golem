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

//! Transaction-scoped guards that order writes across the account tree and plan definitions.
//!
//! A guard protects one account or one plan definition. Every participating writer acquires all
//! of its guards first, before it reads or locks any data row, and then rereads the state it
//! depends on in fresh statements. The order is:
//!
//! 1. account guards, then plan guards;
//! 2. within each class, ascending signed key.
//!
//! Each key appears once: when two entities share a key, the strongest requested mode wins. A
//! transaction never upgrades a shared guard to an exclusive one.
//!
//! An account guard on an affected root also protects that root's subtree: every writer below
//! it takes shared guards on all of its ancestors. A writer that changes the policy of an
//! account therefore takes an exclusive guard on that account and shared guards on its
//! ancestors, which freezes the subtree for its compatibility check without a guard per
//! descendant and without an exclusive guard on the installation root.
//!
//! PostgreSQL uses `pg_advisory_xact_lock` and `pg_advisory_xact_lock_shared` with a bounded
//! `lock_timeout`. A key sets bit 63, so it is negative and never meets the positive registry
//! journal key; bit 62 selects the class; the low 62 bits are the first eight bytes of the
//! SHA-256 of the entity's UUID bytes, big-endian. SQLite write transactions start with
//! `BEGIN IMMEDIATE` and therefore already exclude each other, so SQLite takes no extra lock.

use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::RepoError;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GuardClass {
    Account,
    Plan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GuardMode {
    Shared,
    Exclusive,
}

pub fn guard_key(class: GuardClass, id: Uuid) -> i64 {
    let digest = Sha256::digest(id.as_bytes());
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&digest[..8]);
    let low = u64::from_be_bytes(prefix) & ((1 << 62) - 1);
    let class_bit = match class {
        GuardClass::Account => 0,
        GuardClass::Plan => 1 << 62,
    };
    (1u64 << 63 | class_bit | low) as i64
}

/// The guards one attempt of a write acquires.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardSet {
    guards: BTreeMap<(GuardClass, i64), GuardMode>,
}

impl GuardSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, class: GuardClass, id: Uuid, mode: GuardMode) -> &mut Self {
        self.add_key(class, guard_key(class, id), mode)
    }

    pub fn add_all(
        &mut self,
        class: GuardClass,
        ids: impl IntoIterator<Item = Uuid>,
        mode: GuardMode,
    ) -> &mut Self {
        for id in ids {
            self.add(class, id, mode);
        }
        self
    }

    /// Adds a guard by its key. Colliding keys merge into the strongest mode.
    pub fn add_key(&mut self, class: GuardClass, key: i64, mode: GuardMode) -> &mut Self {
        let entry = self.guards.entry((class, key)).or_insert(mode);
        *entry = (*entry).max(mode);
        self
    }

    /// The guards in acquisition order.
    pub fn ordered(&self) -> Vec<(GuardClass, i64, GuardMode)> {
        self.guards
            .iter()
            .map(|((class, key), mode)| (*class, *key, *mode))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.guards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.guards.is_empty()
    }
}

/// PostgreSQL reports `lock_timeout` as `lock_not_available`.
pub fn is_guard_timeout(error: &RepoError) -> bool {
    match error {
        RepoError::InternalError(err) => err
            .downcast_ref::<sqlx::Error>()
            .and_then(|err| err.as_database_error())
            .and_then(|err| err.code())
            .is_some_and(|code| code == "55P03"),
        _ => false,
    }
}

/// Guard acquisition for one database backend.
pub struct DbGuards<P>(PhantomData<P>);

impl DbGuards<PostgresPool> {
    /// Acquires the guards in order. Each wait is bounded by `timeout`; a timeout fails the
    /// statement, see [`is_guard_timeout`], and leaves the transaction to roll back.
    pub async fn acquire_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        guards: &GuardSet,
        timeout: Duration,
    ) -> Result<(), RepoError> {
        if guards.is_empty() {
            return Ok(());
        }
        tx.execute(sqlx::query(&format!(
            "SET LOCAL lock_timeout = '{}ms'",
            timeout.as_millis().max(1)
        )))
        .await?;
        for (_, key, mode) in guards.ordered() {
            let sql = match mode {
                GuardMode::Shared => "SELECT pg_advisory_xact_lock_shared($1)",
                GuardMode::Exclusive => "SELECT pg_advisory_xact_lock($1)",
            };
            tx.execute(sqlx::query(sql).bind(key)).await?;
        }
        // Later statements in the transaction, such as row locks, keep the server default.
        tx.execute(sqlx::query("SET LOCAL lock_timeout = DEFAULT"))
            .await?;
        Ok(())
    }
}

impl DbGuards<SqlitePool> {
    /// SQLite write transactions already exclude each other, so this takes no lock.
    pub async fn acquire_in_tx(
        _tx: &mut <<SqlitePool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        _guards: &GuardSet,
        _timeout: Duration,
    ) -> Result<(), RepoError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn keys_are_negative_and_separate_classes() {
        let id = Uuid::new_v4();
        let account = guard_key(GuardClass::Account, id);
        let plan = guard_key(GuardClass::Plan, id);
        assert!(account < 0 && plan < 0);
        assert_ne!(account, plan);
        assert_eq!(
            account as u64 & ((1 << 62) - 1),
            plan as u64 & ((1 << 62) - 1)
        );
        assert_eq!(account as u64 >> 62, 0b10);
        assert_eq!(plan as u64 >> 62, 0b11);
    }

    #[test]
    fn key_is_the_masked_sha256_prefix() {
        let id = Uuid::nil();
        // SHA-256 of sixteen zero bytes starts with 374708fff7719dd5.
        let expected = (0x374708fff7719dd5u64 & ((1 << 62) - 1)) | (1 << 63);
        assert_eq!(guard_key(GuardClass::Account, id) as u64, expected);
    }

    #[test]
    fn colliding_keys_merge_into_the_strongest_mode_and_accounts_come_first() {
        let mut guards = GuardSet::new();
        guards
            .add_key(GuardClass::Plan, -5, GuardMode::Shared)
            .add_key(GuardClass::Account, -3, GuardMode::Shared)
            .add_key(GuardClass::Account, -9, GuardMode::Exclusive)
            .add_key(GuardClass::Account, -3, GuardMode::Exclusive)
            .add_key(GuardClass::Account, -9, GuardMode::Shared);
        assert_eq!(
            guards.ordered(),
            vec![
                (GuardClass::Account, -9, GuardMode::Exclusive),
                (GuardClass::Account, -3, GuardMode::Exclusive),
                (GuardClass::Plan, -5, GuardMode::Shared),
            ]
        );
    }
}
