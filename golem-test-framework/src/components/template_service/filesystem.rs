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

use crate::components::template_service::TemplateService;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

use golem_common::model::TemplateId;
use tracing::{debug, info};
use uuid::Uuid;

pub struct FileSystemTemplateService {
    root: PathBuf,
}

impl FileSystemTemplateService {
    pub fn new(root: &Path) -> Self {
        info!("Using a directory for storing templates: {root:?}");
        Self {
            root: root.to_path_buf(),
        }
    }
}

#[async_trait]
impl TemplateService for FileSystemTemplateService {
    async fn add_template(&self, local_path: &Path) -> TemplateId {
        let uuid = Uuid::new_v4();

        let target_dir = &self.root;
        debug!("Local template store: {target_dir:?}");
        if !target_dir.exists() {
            std::fs::create_dir_all(target_dir).expect("Failed to create template store directory");
        }

        if !local_path.exists() {
            panic!("Source file does not exist: {local_path:?}");
        }

        let _ = std::fs::copy(local_path, target_dir.join(format!("{uuid}-0.wasm")))
            .expect("Failed to copy WASM to the local template store");

        TemplateId(uuid)
    }

    async fn update_template(&self, template_id: &TemplateId, local_path: &Path) -> i32 {
        let target_dir = &self.root;

        debug!("Local template store: {target_dir:?}");
        if !target_dir.exists() {
            std::fs::create_dir_all(target_dir).expect("Failed to create template store directory");
        }

        if !local_path.exists() {
            std::panic!("Source file does not exist: {local_path:?}");
        }

        let last_version = self.get_latest_version(template_id).await;
        let new_version = last_version + 1;
        let target = target_dir.join(format!("{template_id}-{new_version}.wasm"));

        let _ = std::fs::copy(local_path, target)
            .expect("Failed to copy WASM to the local template store");

        new_version
    }

    async fn get_latest_version(&self, template_id: &TemplateId) -> i32 {
        let target_dir = &self.root;

        let template_id_str = template_id.to_string();
        let mut versions = std::fs::read_dir(target_dir)
            .expect("Failed to read template store directory")
            .filter_map(|entry| {
                let entry = entry.unwrap();
                let path = entry.path();
                let file_name = path.file_name().unwrap().to_str().unwrap();

                if file_name.starts_with(&template_id_str) && file_name.ends_with(".wasm") {
                    let version_part = file_name.split('-').last().unwrap();
                    let version_part = version_part[..version_part.len() - 5].to_string();
                    version_part.parse::<i32>().ok()
                } else {
                    None
                }
            })
            .collect::<Vec<i32>>();
        versions.sort();
        *versions.last().unwrap_or(&-1)
    }

    fn private_host(&self) -> &str {
        panic!("No real template service running")
    }

    fn private_http_port(&self) -> u16 {
        panic!("No real template service running")
    }

    fn private_grpc_port(&self) -> u16 {
        panic!("No real template service running")
    }

    fn kill(&self) {}
}
