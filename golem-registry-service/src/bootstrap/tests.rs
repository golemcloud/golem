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
use golem_service_base::config::S3BlobStorageConfig;
use golem_service_base::storage::blob::memory::InMemoryS3BlobStorageFactory;
use std::time::Duration;
use test_r::test;

#[test]
fn s3_startup_failure_stops_registry_construction() {
    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        let config = RegistryServiceConfig {
            db: DbConfig::Sqlite(golem_common::config::DbSqliteConfig {
                database: jail.directory().join("registry.db").display().to_string(),
                max_connections: 1,
                foreign_keys: true,
            }),
            blob_storage: BlobStorageConfig::S3(S3BlobStorageConfig {
                region: "us-east-1".to_string(),
                aws_endpoint_url: Some("http://s3.test".to_string()),
                components_bucket: "registry-startup-only".to_string(),
                object_prefix: "objects-not-the-bucket".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let failure = std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "selected bucket transport refusal",
        );
        let factory = InMemoryS3BlobStorageFactory::failed(failure);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut tasks = tokio::task::JoinSet::new();
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                Services::new_inner(&config, &mut tasks, None, factory),
            )
            .await
            .expect("registry startup did not stop at the S3 failure");
            let error = result
                .err()
                .expect("S3 failure must stop service construction");
            assert_eq!(
                error.to_string(),
                "S3 startup check failed for bucket registry-startup-only"
            );
            assert_eq!(
                error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<std::io::Error>())
                    .unwrap()
                    .kind(),
                std::io::ErrorKind::ConnectionRefused
            );
            assert_eq!(
                error.root_cause().to_string(),
                "selected bucket transport refusal"
            );
            assert!(
                tasks.is_empty(),
                "background services must not start after S3 fails"
            );
        });
        Ok(())
    });
}
