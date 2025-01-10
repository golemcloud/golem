// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use futures::future::BoxFuture;
use golem_service_base::migration::Migrations;
use include_dir::Dir;
use sqlx::error::BoxDynError;
use sqlx::migrate::{Migration, MigrationSource};

#[derive(Debug)]
pub struct SpecificIncludedMigrationsDir<'a> {
    included_dir: &'a Dir<'a>,
    sub_dir_name: String,
}

impl SpecificIncludedMigrationsDir<'_> {
    async fn resolve_impl(self) -> Result<Vec<Migration>, BoxDynError> {
        let temp_dir = tempfile::tempdir().map_err(Box::new)?;
        let sub_dir = temp_dir.path().join(self.sub_dir_name);

        // extract assumes that the directory the entries will be extracted to already exists.
        tokio::fs::create_dir(&sub_dir).await?;
        self.included_dir
            .extract(temp_dir.path())
            .map_err(Box::new)?;

        sub_dir.resolve().await
    }
}

impl<'a> MigrationSource<'a> for SpecificIncludedMigrationsDir<'a> {
    fn resolve(self) -> BoxFuture<'a, Result<Vec<Migration>, BoxDynError>> {
        Box::pin(self.resolve_impl())
    }
}

pub struct IncludedMigrationsDir(Dir<'static>);

impl IncludedMigrationsDir {
    pub fn new(dir: Dir<'static>) -> Self {
        Self(dir)
    }
}

impl Migrations for IncludedMigrationsDir {
    type Output<'b>
        = SpecificIncludedMigrationsDir<'b>
    where
        Self: 'b;

    fn sqlite_migrations(&self) -> Self::Output<'_> {
        let sub_dir_name = "sqlite".to_string();
        SpecificIncludedMigrationsDir {
            included_dir: self.0.get_dir(&sub_dir_name).unwrap(),
            sub_dir_name,
        }
    }

    fn postgres_migrations(&self) -> Self::Output<'_> {
        let sub_dir_name = "postgres".to_string();
        SpecificIncludedMigrationsDir {
            included_dir: self.0.get_dir(&sub_dir_name).unwrap(),
            sub_dir_name,
        }
    }
}
