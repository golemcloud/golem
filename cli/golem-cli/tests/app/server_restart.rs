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

use crate::Tracing;
use crate::app::TestContext;
use crate::workspace_path;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::Path;
use std::time::{Duration, Instant};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

fn registry_migration_count() -> i64 {
    std::fs::read_dir(workspace_path().join("golem-registry-service/db/migration/sqlite"))
        .expect("failed to read the registry sqlite migrations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sql"))
        .count() as i64
}

async fn registry_migrations_applied(registry_db: &Path, expected: i64) -> bool {
    if !registry_db.exists() {
        return false;
    }
    let Ok(pool) = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(registry_db)
                .read_only(true),
        )
        .await
    else {
        return false;
    };
    let applied = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await;
    pool.close().await;
    applied.is_ok_and(|applied| applied == expected)
}

impl TestContext {
    async fn interrupt_server_during_startup(&mut self) {
        let registry_db = self.data_dir.path().join("registry.db");
        let expected_migrations = registry_migration_count();
        let start = Instant::now();
        while !registry_migrations_applied(&registry_db, expected_migrations).await {
            let server_process = self
                .server_process
                .as_mut()
                .expect("server process should be running while waiting for the registry database");
            if let Some(exit_status) = server_process
                .try_wait()
                .expect("failed to check golem server process status")
            {
                panic!(
                    "Golem server exited before it migrated the registry database: {exit_status}"
                );
            }
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "timed out waiting for the registry migrations of {}",
                registry_db.display()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            !self.ports_file.exists(),
            "the server finished its startup before it could be interrupted"
        );

        let server_process = self
            .server_process
            .as_mut()
            .expect("server process should be running");
        let pid = server_process
            .id()
            .expect("golem server process has no process id");
        let status = tokio::process::Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .status()
            .await
            .expect("failed to run kill");
        assert!(status.success(), "failed to send SIGINT to the server");

        let exit_status = tokio::time::timeout(Duration::from_secs(60), server_process.wait())
            .await
            .expect("timed out waiting for the interrupted server to exit")
            .expect("failed to wait for the interrupted server");
        self.server_process = None;
        println!("> interrupted server exited: {exit_status}");
        assert!(
            !self.ports_file.exists(),
            "the server finished its startup before it was interrupted"
        );
    }
}

#[test]
#[timeout("3m")]
async fn server_restarts_in_data_dir_after_interrupted_first_start(_tracing: &Tracing) {
    let mut ctx = TestContext::new();

    ctx.spawn_server();
    ctx.interrupt_server_during_startup().await;

    ctx.start_server().await;
}
