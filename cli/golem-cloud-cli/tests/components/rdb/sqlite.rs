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

use crate::components::rdb::{DbInfo, Rdb};
use std::path::{Path, PathBuf};
use tracing::info;

pub struct SqliteRdb {
    path: PathBuf,
}

impl SqliteRdb {
    pub fn new(path: &Path) -> Self {
        info!("Using SQLite at path {path:?}");
        if path.exists() {
            std::fs::remove_dir_all(path).expect("Failed to remove Sqlite database file");
        }
        std::fs::create_dir_all(path).expect("Create db dir");

        Self {
            path: path.to_path_buf(),
        }
    }
}

impl Rdb for SqliteRdb {
    fn info(&self) -> DbInfo {
        DbInfo::Sqlite(self.path.clone())
    }

    fn kill(&self) {
        if self.path.exists() {
            info!("Removing Sqlite database file: {:?}", self.path);
            std::fs::remove_dir_all(&self.path).expect("Failed to remove Sqlite database file");
        }
    }
}

impl Drop for SqliteRdb {
    fn drop(&mut self) {
        self.kill();
    }
}
