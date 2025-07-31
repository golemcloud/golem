// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use futures::future::BoxFuture;
use include_dir::Dir;
use sqlx::{
    error::BoxDynError,
    migrate::{Migration, MigrationSource},
};
use std::path::PathBuf;

pub trait Migrations {
    type Output<'a>: MigrationSource<'a>
    where
        Self: 'a;

    fn sqlite_migrations(&self) -> Self::Output<'_>;
    fn postgres_migrations(&self) -> Self::Output<'_>;
}

#[derive(Debug)]
pub struct SpecificMigrationsDir<'a> {
    value: PathBuf,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

impl<'a> MigrationSource<'a> for SpecificMigrationsDir<'a> {
    fn resolve(self) -> BoxFuture<'a, Result<Vec<Migration>, BoxDynError>> {
        self.value.resolve()
    }
}

pub struct MigrationsDir(PathBuf);

impl MigrationsDir {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }
}

impl Migrations for MigrationsDir {
    type Output<'a>
        = SpecificMigrationsDir<'a>
    where
        Self: 'a;

    fn sqlite_migrations(&self) -> Self::Output<'_> {
        SpecificMigrationsDir {
            value: self.0.join("sqlite"),
            _lifetime: std::marker::PhantomData,
        }
    }

    fn postgres_migrations(&self) -> Self::Output<'_> {
        SpecificMigrationsDir {
            value: self.0.join("postgres"),
            _lifetime: std::marker::PhantomData,
        }
    }
}

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

pub struct IncludedMigrationsDir<'a>(&'a Dir<'static>);

impl<'a> IncludedMigrationsDir<'a> {
    pub fn new(dir: &'a Dir<'static>) -> Self {
        Self(dir)
    }
}

impl<'a> Migrations for IncludedMigrationsDir<'a> {
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
