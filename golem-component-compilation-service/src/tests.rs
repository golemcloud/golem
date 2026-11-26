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
use test_r::test;

#[test]
#[test_r::timeout("5s")]
async fn s3_startup_failure_stops_compilation_before_listeners() {
    let config = ServerConfig {
        blob_storage: BlobStorageConfig::S3(S3BlobStorageConfig {
            region: "us-east-1".to_string(),
            aws_endpoint_url: Some("http://s3.test".to_string()),
            compilation_cache_bucket: "compilation-startup-only".to_string(),
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
    let mut tasks = JoinSet::new();
    let error = run_with_blob_storage_factory(config, Registry::new(), &mut tasks, factory)
        .await
        .err()
        .expect("S3 failure must stop service construction");
    assert_eq!(
        error.to_string(),
        "S3 startup check failed for bucket compilation-startup-only"
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
    assert!(tasks.is_empty(), "listeners must not start after S3 fails");
}
