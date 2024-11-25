// Copyright 2024 Golem Cloud
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

use std::path::PathBuf;

use sqlx::migrate::MigrationSource;

pub trait Migrations {
    type Output<'a>: MigrationSource<'a>
        where Self: 'a;

    fn sqlite_migrations<'a>(&'a self) -> Self::Output<'a>;
    fn postgres_migrations<'a>(&'a self) -> Self::Output<'a>;
}

pub struct MigrationsDir(PathBuf);

impl MigrationsDir {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }
}


impl Migrations for MigrationsDir {
    type Output<'a> = PathBuf;

    fn sqlite_migrations<'a>(&'a self) -> Self::Output<'a> {
        self.0.join("sqlite")
    }

    fn postgres_migrations<'a>(&'a self) -> Self::Output<'a> {
        self.0.join("postgres")
    }
}
