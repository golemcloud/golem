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

use crate::components::rdb::{DbInfo, Rdb};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tracing::info;

/// A non-owning SQLite `Rdb` handle that wraps a parent-prepared
/// on-disk path.
///
/// Use this from a `HostedDep::from_descriptor` reconstruction (the
/// `EnvBasedTestDependencies` worker-side path) where the parent already
/// owns and manages the SQLite database directory. Unlike [`super::sqlite::SqliteRdb`]:
///
/// * `new` does **not** clear / recreate the directory.
/// * [`Rdb::kill`] is a no-op.
/// * [`Drop`] is a no-op.
///
/// That is the only safety property we need on the worker side: if a
/// worker subprocess accidentally calls `remove_dir_all` on the path,
/// the parent's live database is gone.
pub struct BorrowedSqliteRdb {
    path: PathBuf,
}

impl BorrowedSqliteRdb {
    pub fn new(path: &Path) -> Self {
        info!("Attaching to existing SQLite database at path {path:?}");
        Self {
            path: path.to_path_buf(),
        }
    }
}

#[async_trait]
impl Rdb for BorrowedSqliteRdb {
    fn info(&self) -> DbInfo {
        DbInfo::Sqlite(self.path.clone())
    }

    async fn kill(&self) {
        // Intentionally a no-op: the parent owns the underlying SQLite
        // directory in `Hosted` reconstruction and is responsible for its
        // lifecycle.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn info_round_trips_path() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("sqlite.db");
        std::fs::write(&db_path, b"").unwrap();

        let rdb = BorrowedSqliteRdb::new(&db_path);
        match rdb.info() {
            DbInfo::Sqlite(p) => assert_eq!(p, db_path),
            other => panic!("expected DbInfo::Sqlite, got {other:?}"),
        }
    }

    #[test]
    fn drop_does_not_delete_underlying_path() {
        // Simulates the worker-side reconstruction: dropping the borrowed
        // handle must NOT delete the parent-owned database file/directory.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("sqlite.db");
        std::fs::write(&db_path, b"parent-owned").unwrap();

        {
            let _rdb = BorrowedSqliteRdb::new(&db_path);
        }

        assert!(
            db_path.exists(),
            "BorrowedSqliteRdb::drop must not delete the parent-owned database file",
        );
        let contents = std::fs::read(&db_path).unwrap();
        assert_eq!(contents, b"parent-owned");
    }

    #[test]
    async fn kill_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("sqlite.db");
        std::fs::write(&db_path, b"parent-owned").unwrap();

        let rdb = BorrowedSqliteRdb::new(&db_path);
        rdb.kill().await;

        assert!(
            db_path.exists(),
            "BorrowedSqliteRdb::kill must not delete the parent-owned database file",
        );
    }
}
