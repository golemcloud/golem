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

use std::path::{Path, PathBuf};
use tokio::{fs, io};

pub struct TempDir {
    path: PathBuf,
    removed: bool,
}

impl TempDir {
    #[must_use = "call the `remove` method when the dir is no longer needed"]
    pub async fn create_new() -> io::Result<Self> {
        let path = PathBuf::from("golem_temp");

        if let Err(error) = fs::remove_dir_all(&path).await {
            if !matches!(error.kind(), io::ErrorKind::NotFound) {
                Err(error)?
            }
        }
        fs::create_dir(&path).await?;

        Ok(Self {
            path,
            removed: false,
        })
    }

    pub async fn remove(mut self) -> io::Result<()> {
        fs::remove_dir_all(&self.path).await?;
        self.removed = true;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.removed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
