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

use super::*;
use crate::sandbox_filesystem::{SandboxFilesystemAdapter, SandboxFollow, SandboxPath};
use test_r::test;

#[test]
async fn sandboxes_on_one_volume_have_independent_root_capabilities() {
    let root = tempfile::tempdir().unwrap();
    let volume = FilesystemVolume::open(
        &crate::services::golem_config::FilesystemStorageMode::Directory {
            root: root.path().into(),
        },
        RetryConfig::default(),
    )
    .unwrap();
    let first_provisioning =
        SandboxFilesystemProvisioning::on_volume(volume.clone(), RetryConfig::default());
    let second_provisioning =
        SandboxFilesystemProvisioning::on_volume(volume, RetryConfig::default());
    let name = |name: &str| {
        SandboxFilesystemName::new("environment".into(), "component".into(), name.into()).unwrap()
    };
    let first = first_provisioning
        .create_fresh(name("first"))
        .await
        .unwrap();
    let second = second_provisioning
        .create_fresh(name("second"))
        .await
        .unwrap();
    std::fs::write(second.root().join("secret"), b"second sandbox").unwrap();

    let error = first
        .get_path_attributes(SandboxPath::at_root("../second/secret"), SandboxFollow::Yes)
        .await
        .unwrap_err();
    assert!(error.is_sandbox_escape(), "{error:?}");
    first.delete_and_verify().await.unwrap();
    assert_eq!(
        std::fs::read(second.root().join("secret")).unwrap(),
        b"second sandbox"
    );
    second.delete_and_verify().await.unwrap();
    assert!(root.path().is_dir());
}

#[test]
fn native_name_rejects_a_component_that_starts_with_a_dot() {
    [
        [".environment", "component", "filesystem"],
        ["environment", ".component", "filesystem"],
        ["environment", "component", ".filesystem"],
    ]
    .into_iter()
    .for_each(|[environment, component, filesystem]| {
        assert!(
            SandboxFilesystemName::new(
                environment.to_string(),
                component.to_string(),
                filesystem.to_string()
            )
            .is_err(),
            "{environment}/{component}/{filesystem} must be refused"
        );
    });
    assert!(
        SandboxFilesystemName::new(
            "environment.".to_string(),
            "compo.nent".to_string(),
            "filesystem".to_string()
        )
        .is_ok()
    );
}

#[test]
fn native_name_rejects_path_components() {
    assert!(
        SandboxFilesystemName::new("..".to_string(), "component".to_string(), "fs".to_string())
            .is_err()
    );
    assert!(
        SandboxFilesystemName::new(
            "environment".to_string(),
            "a/b".to_string(),
            "fs".to_string()
        )
        .is_err()
    );
}
