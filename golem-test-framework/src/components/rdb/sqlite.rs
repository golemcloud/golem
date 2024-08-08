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
        remove_path(path);

        let dir = if path.is_dir() {
            Some(path)
        } else {
            path.parent()
        };

        if let Some(dir) = dir {
            std::fs::create_dir_all(dir).expect("Failed to create SQLite database directory");
        }

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
        remove_path(&self.path);
    }
}

fn remove_path(path: &Path) {
    if path.exists() {
        let res = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };

        if let Err(e) = res {
            tracing::error!("Failed to remove SQLite database at path {path:?}: {e}");
        }
    }
}

impl Drop for SqliteRdb {
    fn drop(&mut self) {
        self.kill();
    }
}
