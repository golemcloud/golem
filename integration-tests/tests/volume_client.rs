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

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{BucketVersioningStatus, ServerSideEncryption, VersioningConfiguration};
use bytes::Bytes;
use futures::{StreamExt, stream};
use golem_service_base::aws::AwsCredentials;
use golem_service_base::volume::connection::ConnectionPolicy;
use golem_service_base::volume::s3::{
    Encryption, ObjectMetadata, S3Volume, S3VolumeConfig, VolumeError, WriteCondition, WriteTarget,
};
use golem_test_framework::components::s3::{DockerRustFs, S3Server};
use test_r::{test, timeout};

test_r::enable!();

async fn versioned_store() -> (DockerRustFs, aws_sdk_s3::Client, Box<str>) {
    let server = DockerRustFs::new().await;
    let setup = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(server.endpoint())
            .force_path_style(true)
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                server.access_key_id(),
                server.secret_access_key(),
                None,
                None,
                "test",
            ))
            .build(),
    );
    let bucket = format!("volume-test-{}", uuid::Uuid::new_v4()).into_boxed_str();
    setup
        .create_bucket()
        .bucket(bucket.as_ref())
        .send()
        .await
        .unwrap();
    setup
        .put_bucket_versioning()
        .bucket(bucket.as_ref())
        .versioning_configuration(
            VersioningConfiguration::builder()
                .status(BucketVersioningStatus::Enabled)
                .build(),
        )
        .send()
        .await
        .unwrap();
    let encryption = setup
        .get_bucket_encryption()
        .bucket(bucket.as_ref())
        .send()
        .await
        .unwrap_err();
    assert_eq!(
        encryption
            .as_service_error()
            .and_then(ProvideErrorMetadata::code),
        Some("ServerSideEncryptionConfigurationNotFoundError")
    );
    (server, setup, bucket)
}

fn volume(server: &DockerRustFs, bucket: &str, encryption: Option<Encryption>) -> S3Volume {
    let credentials = AwsCredentials::from_record(&serde_json::json!({
        "accessKeyId": server.access_key_id(), "secretAccessKey": server.secret_access_key()
    }))
    .unwrap();
    S3Volume::new(
        S3VolumeConfig {
            endpoint: Some(server.endpoint().into_boxed_str()),
            region: "us-east-1".into(),
            bucket: bucket.into(),
            prefix: "corpus/".into(),
            encryption,
        },
        credentials,
        ConnectionPolicy {
            allow_loopback: true,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
#[timeout("180s")]
async fn rustfs_enforces_volume_preconditions_versions_and_multipart_completion() {
    let (server, setup, bucket) = versioned_store().await;
    let volume = volume(&server, &bucket, None);

    assert_eq!(
        volume
            .versioning()
            .await
            .unwrap()
            .map(|status| status.into_payload()),
        Some(BucketVersioningStatus::Enabled)
    );
    assert!(volume.lifecycle().await.unwrap().is_empty());
    let key = WriteTarget::File("document");
    let first = volume
        .put(
            key,
            WriteCondition::Absent,
            Bytes::from_static(b"first-content"),
        )
        .await
        .unwrap();
    let first_etag = first.etag.as_ref();
    let first_version = first.version_id.as_deref().unwrap();
    assert_ne!(first_version, "null");
    assert_eq!(
        volume.put(key, WriteCondition::Absent, Bytes::new()).await,
        Err(VolumeError::Exists)
    );
    let second = volume
        .put(
            key,
            WriteCondition::Matches(first_etag),
            Bytes::from_static(b"second-content"),
        )
        .await
        .unwrap();
    assert_ne!(first.etag, second.etag);
    assert_ne!(first.version_id, second.version_id);
    assert_eq!(
        volume
            .put(
                key,
                WriteCondition::Matches(first_etag),
                Bytes::from_static(b"must-not-win")
            )
            .await,
        Err(VolumeError::Conflict)
    );
    assert_eq!(
        volume.delete(key, first_etag, 13).await,
        Err(VolumeError::Conflict)
    );
    let current = volume.read("document", None, None, None).await.unwrap();
    assert_eq!(current.body.collect().await.unwrap(), "second-content");
    let old = volume
        .read("document", Some(first_version), None, Some(1..=4))
        .await
        .unwrap();
    assert_eq!(old.version, first);
    assert_eq!(old.body.collect().await.unwrap(), "irst");

    // A marker comes from another S3 client. The volume never creates one.
    setup
        .put_object()
        .bucket(bucket.as_ref())
        .key("corpus/dir/")
        .body(ByteStream::from_static(b""))
        .send()
        .await
        .unwrap();
    let empty_file = volume
        .put(
            WriteTarget::File("dir/file"),
            WriteCondition::Absent,
            Bytes::new(),
        )
        .await
        .unwrap();
    let page = volume.list("", None).await.unwrap();
    assert_eq!(
        page.objects.as_ref(),
        [
            ObjectMetadata {
                key: "dir/file".into(),
                size: 0,
                version: empty_file
            },
            ObjectMetadata {
                key: "document".into(),
                size: 14,
                version: second.clone()
            },
        ]
    );

    let upload = volume.start_multipart(key).await.unwrap();
    let part = volume
        .upload_part(
            key,
            upload.payload(),
            1,
            Bytes::from_static(b"multipart-content"),
        )
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(
                key,
                upload.payload(),
                &[part],
                WriteCondition::Matches(first_etag)
            )
            .await,
        Err(VolumeError::Conflict)
    );
    volume.abort_multipart(key, upload.payload()).await.unwrap();
    let current = volume.read("document", None, None, None).await.unwrap();
    assert_eq!(current.body.collect().await.unwrap(), "second-content");

    let upload = volume.start_multipart(key).await.unwrap();
    let part = volume
        .upload_part(
            key,
            upload.payload(),
            1,
            Bytes::from_static(b"multipart-content"),
        )
        .await
        .unwrap();
    let published = volume
        .complete_multipart(
            key,
            upload.payload(),
            &[part],
            WriteCondition::Matches(&second.etag),
        )
        .await
        .unwrap();
    let current = volume.read("document", None, None, None).await.unwrap();
    assert_eq!(current.version, published);
    assert_eq!(current.body.collect().await.unwrap(), "multipart-content");

    let absent = WriteTarget::File("new-large");
    let upload = volume.start_multipart(absent).await.unwrap();
    let part = volume
        .upload_part(absent, upload.payload(), 1, Bytes::from_static(b"whole"))
        .await
        .unwrap();
    volume
        .complete_multipart(absent, upload.payload(), &[part], WriteCondition::Absent)
        .await
        .unwrap();
    let upload = volume.start_multipart(absent).await.unwrap();
    let part = volume
        .upload_part(
            absent,
            upload.payload(),
            1,
            Bytes::from_static(b"cannot-replace"),
        )
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(absent, upload.payload(), &[part], WriteCondition::Absent)
            .await,
        Err(VolumeError::Exists)
    );
    volume
        .abort_multipart(absent, upload.payload())
        .await
        .unwrap();

    let abandoned = WriteTarget::File("abandoned");
    let upload = volume.start_multipart(abandoned).await.unwrap();
    volume
        .upload_part(
            abandoned,
            upload.payload(),
            1,
            Bytes::from_static(b"incomplete"),
        )
        .await
        .unwrap();
    volume
        .abort_multipart(abandoned, upload.payload())
        .await
        .unwrap();
    assert_eq!(
        volume.stat("abandoned", None).await,
        Err(VolumeError::NotFound)
    );
    let uploads = setup
        .list_multipart_uploads()
        .bucket(bucket.as_ref())
        .send()
        .await
        .unwrap();
    assert!(uploads.uploads().is_empty());
    volume.delete(key, &published.etag, 17).await.unwrap();
    assert_eq!(
        volume.stat("document", None).await,
        Err(VolumeError::NotFound)
    );
    server.kill().await;
}

async fn encrypted_roundtrips(
    encryption: Encryption,
    algorithm: ServerSideEncryption,
    kms: Option<&str>,
) {
    let (server, setup, bucket) = versioned_store().await;
    let volume = volume(&server, &bucket, Some(encryption));
    let single = Bytes::from_static(b"encrypted single object, not an empty marker");
    let published = volume
        .put(
            WriteTarget::File("single"),
            WriteCondition::Absent,
            single.clone(),
        )
        .await
        .unwrap();
    let multipart = Bytes::from(vec![0x6d; 10 * 1024 * 1024]);
    let target = WriteTarget::File("multipart");
    let upload = volume.start_multipart(target).await.unwrap();
    let parts = stream::iter([1, 2])
        .fold(Vec::with_capacity(2), |mut parts, number| {
            let volume = &volume;
            let upload = &upload;
            let start = (number as usize - 1) * 5 * 1024 * 1024;
            let body = multipart.slice(start..start + 5 * 1024 * 1024);
            async move {
                parts.push(
                    volume
                        .upload_part(target, upload.payload(), number, body)
                        .await
                        .unwrap(),
                );
                parts
            }
        })
        .await
        .into_boxed_slice();
    let completed = volume
        .complete_multipart(target, upload.payload(), &parts, WriteCondition::Absent)
        .await
        .unwrap();
    stream::iter([
        ("single", single, published),
        ("multipart", multipart, completed),
    ])
    .then(|(key, body, published)| {
        let volume = &volume;
        let setup = &setup;
        let bucket = &bucket;
        let algorithm = &algorithm;
        async move {
            let head = setup
                .head_object()
                .bucket(bucket.as_ref())
                .key(format!("corpus/{key}"))
                .send()
                .await
                .unwrap();
            assert_eq!(head.server_side_encryption(), Some(algorithm));
            assert_eq!(head.ssekms_key_id(), kms);
            assert_eq!(head.content_length(), Some(body.len() as i64));
            assert_eq!(head.e_tag(), Some(published.etag.as_ref()));
            assert!(published.version_id.is_some());
            assert_eq!(head.version_id(), published.version_id.as_deref());
            let read = volume
                .read(key, published.version_id.as_deref(), None, None)
                .await
                .unwrap();
            assert_eq!(read.version, published);
            assert_eq!(read.body.collect().await.unwrap(), body);
        }
    })
    .collect::<()>()
    .await;
    server.kill().await;
}

#[test]
#[timeout("180s")]
async fn rustfs_sse_s3_preserves_single_and_multipart_objects() {
    encrypted_roundtrips(Encryption::S3, ServerSideEncryption::Aes256, None).await;
}

#[test]
#[timeout("180s")]
async fn rustfs_sse_kms_preserves_single_and_multipart_objects() {
    encrypted_roundtrips(
        Encryption::Kms {
            key_id: "volume-test-key".into(),
        },
        ServerSideEncryption::AwsKms,
        Some("volume-test-key"),
    )
    .await;
}

#[test]
#[timeout("180s")]
async fn rustfs_sse_kms_rejects_a_missing_key_without_publishing() {
    let (server, setup, bucket) = versioned_store().await;
    let volume = volume(
        &server,
        &bucket,
        Some(Encryption::Kms {
            key_id: "missing-volume-key".into(),
        }),
    );
    assert_eq!(
        volume
            .put(
                WriteTarget::File("missing-kms"),
                WriteCondition::Absent,
                Bytes::from_static(b"must not publish")
            )
            .await,
        Err(VolumeError::RequestFailed)
    );
    assert_eq!(
        volume.stat("missing-kms", None).await,
        Err(VolumeError::NotFound)
    );
    let head = setup
        .head_object()
        .bucket(bucket.as_ref())
        .key("corpus/missing-kms")
        .send()
        .await
        .unwrap_err();
    assert_eq!(head.raw_response().unwrap().status().as_u16(), 404);
    server.kill().await;
}
