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
use futures::{FutureExt, StreamExt, stream};
use golem_service_base::aws::AwsCredentials;
use golem_service_base::volume::connection::ConnectionPolicy;
use golem_service_base::volume::memory::InMemoryVolume;
use golem_service_base::volume::s3::{Encryption, S3Volume, S3VolumeConfig};
use golem_service_base::volume::{
    BucketVersioning, LifecycleRule, ListedObject, ObjectVersion, VolumeClient, VolumeError,
    WriteCondition, WriteTarget,
};
use golem_test_framework::components::s3::{DockerRustFs, S3Server};
use std::ops::RangeInclusive;
use std::panic::{AssertUnwindSafe, resume_unwind};
use std::sync::Arc;
use std::time::Duration;
use test_r::{test, timeout};

test_r::enable!();

async fn store(versioned: bool) -> (DockerRustFs, aws_sdk_s3::Client, Box<str>) {
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
    if versioned {
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
    }
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

async fn rustfs_case<F>(versioned: bool, behavior: F)
where
    F: for<'a> AsyncFnOnce(&'a DockerRustFs, &'a aws_sdk_s3::Client, &'a str),
{
    // These async deadlines do not bound synchronous Testcontainers destruction.
    let (server, setup, bucket) = match AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(600), store(versioned)).await
    })
    .catch_unwind()
    .await
    {
        Ok(Ok(store)) => store,
        Ok(Err(_)) => panic!("RustFS setup timed out after 600 seconds"),
        Err(panic) => {
            eprintln!("RustFS setup panicked");
            resume_unwind(panic);
        }
    };
    let behavior = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(180), behavior(&server, &setup, &bucket)).await
    })
    .catch_unwind()
    .await;
    let cleanup = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(180), server.kill()).await
    })
    .catch_unwind()
    .await;
    let cleanup_failure = match &cleanup {
        Ok(Ok(())) => None,
        Ok(Err(_)) => Some("RustFS cleanup timed out after 180 seconds"),
        Err(_) => Some("RustFS cleanup panicked"),
    };
    match behavior {
        Err(panic) => {
            eprintln!("RustFS behavior panicked");
            if let Some(failure) = cleanup_failure {
                eprintln!("{failure}");
            }
            resume_unwind(panic);
        }
        Ok(Err(_)) => match cleanup_failure {
            Some(failure) => {
                panic!("RustFS behavior timed out after 180 seconds; {failure}");
            }
            None => panic!("RustFS behavior timed out after 180 seconds"),
        },
        Ok(Ok(())) => match cleanup {
            Ok(Ok(())) => {}
            Ok(Err(_)) => panic!("RustFS cleanup timed out after 180 seconds"),
            Err(panic) => {
                eprintln!("RustFS cleanup panicked");
                resume_unwind(panic);
            }
        },
    }
}

fn volume(
    server: &DockerRustFs,
    bucket: &str,
    prefix: &str,
    encryption: Option<Encryption>,
) -> S3Volume {
    let credentials = AwsCredentials::from_record(&serde_json::json!({
        "accessKeyId": server.access_key_id(), "secretAccessKey": server.secret_access_key()
    }))
    .unwrap();
    S3Volume::new(
        S3VolumeConfig {
            endpoint: Some(server.endpoint().into_boxed_str()),
            region: "us-east-1".into(),
            bucket: bucket.into(),
            prefix: prefix.into(),
            encryption,
        },
        credentials,
        ConnectionPolicy {
            allow_loopback: true,
            ..ConnectionPolicy::new(Box::new([]))
        },
    )
    .unwrap()
}

async fn assert_read(
    volume: &dyn VolumeClient,
    key: &str,
    reference: &ObjectVersion,
    range: Option<RangeInclusive<u64>>,
    expected: &[u8],
) {
    let read = volume.read(key, reference, range).await.unwrap();
    assert_eq!(&read.version, reference);
    assert_eq!(read.length, expected.len() as u64);
    assert_eq!(read.body.collect().await.unwrap().as_ref(), expected);
}

async fn volume_contract(volume: &dyn VolumeClient, versioned: bool) {
    assert_eq!(
        volume.versioning().await.unwrap(),
        if versioned {
            BucketVersioning::Enabled
        } else {
            BucketVersioning::Unversioned
        }
    );
    let lifecycle: Box<[LifecycleRule]> = volume.lifecycle().await.unwrap();
    assert!(lifecycle.is_empty());
    preconditions_versions_and_history(volume, versioned).await;
    listing_and_ranges(volume).await;
    listing_continuations(volume).await;
    multipart_completion_and_abort(volume).await;
    multipart_session_identity(volume).await;
    multipart_part_identity_and_sizes(volume, versioned).await;
    probes_and_invalid_paths(volume, versioned).await;
}

async fn preconditions_versions_and_history(volume: &dyn VolumeClient, versioned: bool) {
    async fn assert_marker_selection(volume: &dyn VolumeClient, marker: &str) {
        assert_eq!(
            volume.stat("document", Some(marker)).await,
            Err(VolumeError::Unsupported)
        );
        assert_eq!(
            volume
                .stat("document", Some("7d52c7e1-1d62-4fd7-8575-43ebdcab6b5c"))
                .await,
            Err(VolumeError::NotFound)
        );
        assert_eq!(
            volume.stat("other-key", Some(marker)).await,
            Err(VolumeError::NotFound)
        );
        let reference = ObjectVersion {
            etag: "\"marker-read-a7Z!\"".into(),
            version_id: Some(marker.into()),
        };
        stream::iter([None, Some(103..=207)])
            .then(|range| {
                let reference = &reference;
                async move {
                    assert_eq!(
                        volume.read("document", reference, range).await.unwrap_err(),
                        VolumeError::Unsupported
                    );
                }
            })
            .collect::<()>()
            .await;
        assert_eq!(
            volume
                .read("other-key", &reference, None)
                .await
                .unwrap_err(),
            VolumeError::NotFound
        );
        assert_eq!(
            volume
                .read("document", &reference, Some(RangeInclusive::new(9, 8)))
                .await
                .unwrap_err(),
            VolumeError::InvalidRequest
        );
        let weak = ObjectVersion {
            etag: "W/\"marker-weak-a7Z!\"".into(),
            version_id: reference.version_id,
        };
        assert_eq!(
            volume.read("document", &weak, None).await.unwrap_err(),
            VolumeError::InvalidRequest
        );
    }

    let key = WriteTarget::File("document");
    assert_eq!(
        volume.stat("document", None).await,
        Err(VolumeError::NotFound)
    );
    assert_eq!(
        volume
            .put(
                key,
                WriteCondition::Matches("\"missing\""),
                Bytes::from_static(b"must-not-create")
            )
            .await,
        Err(VolumeError::Conflict)
    );
    let first = volume
        .put(
            key,
            WriteCondition::Absent,
            Bytes::from_static(b"first-content"),
        )
        .await
        .unwrap();
    let first_etag = first.etag.as_ref();
    assert_eq!(first.version_id.is_some(), versioned);
    assert_ne!(first.version_id.as_deref(), Some("null"));
    let first_metadata = volume.stat("document", None).await.unwrap();
    assert_eq!(first_metadata.key.as_ref(), "document");
    assert_eq!(first_metadata.size, 13);
    assert_eq!(first_metadata.version, first);
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
    assert_eq!(second.version_id.is_some(), versioned);
    if versioned {
        assert_ne!(first.version_id, second.version_id);
    }
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
        volume.delete(key, &first_metadata).await,
        Err(VolumeError::Conflict)
    );
    assert_read(volume, "document", &second, None, b"second-content").await;
    let wrong_etag = ObjectVersion {
        etag: "\"not-the-reference\"".into(),
        version_id: second.version_id.clone(),
    };
    assert_eq!(
        volume
            .read("document", &wrong_etag, None)
            .await
            .unwrap_err(),
        VolumeError::Conflict
    );
    assert_eq!(
        volume
            .read("document", &wrong_etag, Some(100..=200))
            .await
            .unwrap_err(),
        VolumeError::Conflict
    );
    if versioned {
        let without_id = ObjectVersion {
            etag: second.etag.clone(),
            version_id: None,
        };
        stream::iter([None, Some(14..=20)])
            .then(|range| {
                let without_id = &without_id;
                async move {
                    assert_eq!(
                        volume
                            .read("document", without_id, range)
                            .await
                            .unwrap_err(),
                        VolumeError::InvalidResponse
                    );
                }
            })
            .collect::<()>()
            .await;
        assert_eq!(
            volume
                .stat("document", first.version_id.as_deref())
                .await
                .unwrap(),
            first_metadata
        );
        assert_read(volume, "document", &first, Some(1..=4), b"irst").await;
    } else {
        assert_eq!(
            volume.read("document", &first, None).await.unwrap_err(),
            VolumeError::Conflict
        );
        assert_eq!(
            volume
                .read("document", &first, Some(100..=200))
                .await
                .unwrap_err(),
            VolumeError::Conflict
        );
        assert_eq!(
            volume.stat("document", Some("null")).await.unwrap().version,
            second
        );
    }
    let current = volume.stat("document", None).await.unwrap();
    assert_eq!(current.version, second);
    assert_eq!(current.size, 14);
    let marker = volume.delete(key, &current).await.unwrap();
    assert_eq!(marker.is_some(), versioned);
    if let Some(marker) = &marker {
        assert_ne!(marker.payload().as_ref(), "null");
        assert_ne!(Some(marker.payload().as_ref()), first.version_id.as_deref());
        assert_ne!(
            Some(marker.payload().as_ref()),
            second.version_id.as_deref()
        );
    }
    assert_eq!(
        volume.stat("document", None).await,
        Err(VolumeError::NotFound)
    );
    let page = volume.list("document", None).await.unwrap();
    assert!(page.objects.is_empty());
    assert!(page.continuation.is_none());
    if versioned {
        let marker = marker.unwrap();
        assert_marker_selection(volume, marker.payload()).await;
        assert_eq!(
            volume
                .stat("document", second.version_id.as_deref())
                .await
                .unwrap(),
            current
        );
        assert_read(volume, "document", &first, None, b"first-content").await;
        assert_read(volume, "document", &second, None, b"second-content").await;
        assert_read(volume, "document", &second, Some(14..=20), b"").await;
        let recreated = volume
            .put(
                key,
                WriteCondition::Absent,
                Bytes::from_static(b"recreated-content-a7Z!"),
            )
            .await
            .unwrap();
        assert_ne!(recreated.etag, second.etag);
        assert_ne!(recreated.version_id, second.version_id);
        assert!(recreated.version_id.is_some());
        assert_ne!(
            recreated.version_id.as_deref(),
            Some(marker.payload().as_ref())
        );
        assert_marker_selection(volume, marker.payload()).await;
        let metadata = volume.stat("document", None).await.unwrap();
        assert_eq!(metadata.key.as_ref(), "document");
        assert_eq!(metadata.size, 22);
        assert_eq!(metadata.version, recreated);
        assert_read(
            volume,
            "document",
            &recreated,
            None,
            b"recreated-content-a7Z!",
        )
        .await;
        assert_eq!(
            volume
                .list("document", None)
                .await
                .unwrap()
                .objects
                .as_ref(),
            [ListedObject {
                key: "document".into(),
                size: 22,
                etag: recreated.etag,
            }]
        );
        assert_eq!(
            volume
                .stat("document", second.version_id.as_deref())
                .await
                .unwrap(),
            current
        );
        assert_read(volume, "document", &first, None, b"first-content").await;
        assert_read(volume, "document", &second, None, b"second-content").await;
    } else {
        assert_eq!(
            volume.stat("document", Some("null")).await,
            Err(VolumeError::NotFound)
        );
        assert_eq!(
            volume.read("document", &second, None).await.unwrap_err(),
            VolumeError::NotFound
        );
        assert_eq!(
            volume
                .read("document", &second, Some(100..=200))
                .await
                .unwrap_err(),
            VolumeError::NotFound
        );
    }
}

async fn listing_and_ranges(volume: &dyn VolumeClient) {
    let empty_file = volume
        .put(
            WriteTarget::File("listing/empty"),
            WriteCondition::Absent,
            Bytes::new(),
        )
        .await
        .unwrap();
    let file = volume
        .put(
            WriteTarget::File("listing/file"),
            WriteCondition::Absent,
            Bytes::from_static(b"abcdef"),
        )
        .await
        .unwrap();
    let slash_file = volume
        .put(
            WriteTarget::File("listing/slash/"),
            WriteCondition::Absent,
            Bytes::from_static(b"body"),
        )
        .await
        .unwrap();
    let page = volume.list("listing/", None).await.unwrap();
    assert!(page.continuation.is_none());
    assert_eq!(
        page.objects.as_ref(),
        [
            ListedObject {
                key: "listing/empty".into(),
                size: 0,
                etag: empty_file.etag.clone()
            },
            ListedObject {
                key: "listing/file".into(),
                size: 6,
                etag: file.etag.clone()
            },
            ListedObject {
                key: "listing/slash/".into(),
                size: 4,
                etag: slash_file.etag.clone()
            },
        ]
    );
    let metadata = volume.stat("listing/empty", None).await.unwrap();
    assert_eq!(metadata.key.as_ref(), "listing/empty");
    assert_eq!(metadata.size, 0);
    assert_eq!(metadata.version, empty_file);
    assert_read(volume, "listing/empty", &empty_file, None, b"").await;
    assert_read(volume, "listing/empty", &empty_file, Some(0..=0), b"").await;
    assert_read(volume, "listing/empty", &empty_file, Some(1..=10), b"").await;

    let metadata = volume.stat("listing/file", None).await.unwrap();
    assert_eq!(metadata.key.as_ref(), "listing/file");
    assert_eq!(metadata.size, 6);
    assert_eq!(metadata.version, file);
    assert_read(volume, "listing/file", &metadata.version, None, b"abcdef").await;
    assert_read(volume, "listing/file", &file, Some(1..=4), b"bcde").await;
    assert_read(volume, "listing/file", &file, Some(4..=99), b"ef").await;
    assert_read(volume, "listing/file", &file, Some(6..=6), b"").await;
    assert_read(volume, "listing/file", &file, Some(9..=20), b"").await;
    stream::iter([i64::MAX as u64, u64::MAX])
        .then(|start| {
            let file = &file;
            let empty_file = &empty_file;
            async move {
                assert_read(volume, "listing/file", file, Some(start..=u64::MAX), b"").await;
                assert_read(
                    volume,
                    "listing/empty",
                    empty_file,
                    Some(start..=u64::MAX),
                    b"",
                )
                .await;
            }
        })
        .collect::<()>()
        .await;
    let slash_metadata = volume.stat("listing/slash/", None).await.unwrap();
    assert_eq!(slash_metadata.size, 4);
    assert_eq!(slash_metadata.version, slash_file);
    assert_read(volume, "listing/slash/", &slash_file, None, b"body").await;
    assert_read(volume, "listing/slash/", &slash_file, Some(4..=10), b"").await;
}

async fn listing_continuations(volume: &dyn VolumeClient) {
    volume
        .put(
            WriteTarget::File("pages0/foreign"),
            WriteCondition::Absent,
            Bytes::from_static(b"not-in-pages"),
        )
        .await
        .unwrap();
    let expected = stream::iter(0..2003)
        .map(|index| async move {
            let key: Arc<str> = format!("pages/{index:04}").into();
            let version = volume
                .put(
                    WriteTarget::File(&key),
                    WriteCondition::Absent,
                    Bytes::from_static(b"page-body-Q9"),
                )
                .await
                .unwrap();
            ListedObject {
                key,
                size: 12,
                etag: version.etag,
            }
        })
        .buffered(16)
        .collect::<Vec<_>>()
        .await
        .into_boxed_slice();
    let pages = stream::unfold(Some(None::<Box<str>>), |state| async move {
        let continuation = state?;
        let page = volume
            .list("pages/", continuation.as_deref())
            .await
            .unwrap();
        let finished = page.continuation.is_none();
        Some(((page.objects, finished), page.continuation.map(Some)))
    })
    .take(2004)
    .collect::<Vec<_>>()
    .await
    .into_boxed_slice();
    assert!(pages.len() >= 3);
    assert!(pages.last().unwrap().1);
    let actual = pages
        .into_vec()
        .into_iter()
        .flat_map(|(objects, _)| objects.into_vec())
        .collect::<Box<[_]>>();
    assert_eq!(actual, expected);
}

async fn multipart_session_identity(volume: &dyn VolumeClient) {
    let target = WriteTarget::File("multipart-reused-id");
    let aborted = volume.start_multipart(target).await.unwrap();
    let old_part = volume
        .upload_part(
            target,
            aborted.payload(),
            1,
            Bytes::from_static(b"old-part-A7"),
        )
        .await
        .unwrap();
    volume
        .abort_multipart(target, aborted.payload())
        .await
        .unwrap();
    let open = volume.start_multipart(target).await.unwrap();
    assert_ne!(open, aborted);
    let retained = volume
        .upload_part(
            target,
            open.payload(),
            1,
            Bytes::from_static(b"new-part-Q9"),
        )
        .await
        .unwrap();
    assert_eq!(
        volume
            .upload_part(
                target,
                aborted.payload(),
                1,
                Bytes::from_static(b"wrong-id")
            )
            .await,
        Err(VolumeError::NotFound)
    );
    assert_eq!(
        volume
            .complete_multipart(
                target,
                aborted.payload(),
                &[old_part],
                WriteCondition::Absent
            )
            .await,
        Err(VolumeError::NotFound)
    );
    assert_eq!(
        volume.abort_multipart(target, aborted.payload()).await,
        Err(VolumeError::NotFound)
    );
    let published = volume
        .complete_multipart(target, open.payload(), &[retained], WriteCondition::Absent)
        .await
        .unwrap();
    assert_read(
        volume,
        "multipart-reused-id",
        &published,
        None,
        b"new-part-Q9",
    )
    .await;

    let sessions = stream::iter([
        ("multipart-id-a", b"retained-A7".as_slice()),
        ("multipart-id-b", b"retained-B9".as_slice()),
    ])
    .then(|(key, body)| async move {
        let target = WriteTarget::File(key);
        let upload = volume.start_multipart(target).await.unwrap();
        let part = volume
            .upload_part(target, upload.payload(), 1, Bytes::from_static(body))
            .await
            .unwrap();
        (key, upload, part, body)
    })
    .collect::<Vec<_>>()
    .await
    .into_boxed_slice();
    let (a, b) = (&sessions[0], &sessions[1]);
    assert_ne!(a.1, b.1);
    let wrong_target = WriteTarget::File(b.0);
    assert_eq!(
        volume
            .upload_part(
                wrong_target,
                a.1.payload(),
                1,
                Bytes::from_static(b"wrong-key")
            )
            .await,
        Err(VolumeError::NotFound)
    );
    assert_eq!(
        volume
            .complete_multipart(
                wrong_target,
                a.1.payload(),
                std::slice::from_ref(&a.2),
                WriteCondition::Absent,
            )
            .await,
        Err(VolumeError::NotFound)
    );
    assert_eq!(
        volume.abort_multipart(wrong_target, a.1.payload()).await,
        Err(VolumeError::NotFound)
    );
    stream::iter(sessions.iter())
        .then(|(key, upload, part, body)| async move {
            let target = WriteTarget::File(key);
            let parts = std::slice::from_ref(part);
            let published = volume
                .complete_multipart(target, upload.payload(), parts, WriteCondition::Absent)
                .await
                .unwrap();
            assert_eq!(volume.stat(key, None).await.unwrap().version, published);
            assert_read(volume, key, &published, None, body).await;
            assert_eq!(
                volume
                    .complete_multipart(
                        WriteTarget::File("multipart-wrong-replay"),
                        upload.payload(),
                        parts,
                        WriteCondition::Absent,
                    )
                    .await,
                Err(VolumeError::NotFound)
            );
            assert_eq!(
                volume
                    .complete_multipart(
                        target,
                        upload.payload(),
                        parts,
                        WriteCondition::Matches(&published.etag),
                    )
                    .await
                    .unwrap(),
                published
            );
            assert_read(volume, key, &published, None, body).await;
        })
        .collect::<()>()
        .await;
    assert_eq!(
        volume.stat("multipart-wrong-replay", None).await,
        Err(VolumeError::NotFound)
    );
}

async fn multipart_completion_and_abort(volume: &dyn VolumeClient) {
    let key = WriteTarget::File("multipart-document");
    let first = volume
        .put(
            key,
            WriteCondition::Absent,
            Bytes::from_static(b"first-content"),
        )
        .await
        .unwrap();
    let second = volume
        .put(
            key,
            WriteCondition::Matches(&first.etag),
            Bytes::from_static(b"second-content"),
        )
        .await
        .unwrap();

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
    assert_eq!(part.number, 1);
    assert_eq!(part.size, 17);
    assert_eq!(
        volume
            .complete_multipart(
                key,
                upload.payload(),
                &[part],
                WriteCondition::Matches(&first.etag)
            )
            .await,
        Err(VolumeError::Conflict)
    );
    volume.abort_multipart(key, upload.payload()).await.unwrap();
    assert_read(
        volume,
        "multipart-document",
        &second,
        None,
        b"second-content",
    )
    .await;

    let upload = volume.start_multipart(key).await.unwrap();
    let mut part = volume
        .upload_part(
            key,
            upload.payload(),
            1,
            Bytes::from_static(b"multipart-content"),
        )
        .await
        .unwrap();
    part.size += 1;
    let published = volume
        .complete_multipart(
            key,
            upload.payload(),
            &[part],
            WriteCondition::Matches(&second.etag),
        )
        .await
        .unwrap();
    assert_eq!(
        volume
            .stat("multipart-document", None)
            .await
            .unwrap()
            .version,
        published
    );
    assert_read(
        volume,
        "multipart-document",
        &published,
        None,
        b"multipart-content",
    )
    .await;

    let absent = WriteTarget::File("new-large");
    let upload = volume.start_multipart(absent).await.unwrap();
    let part = volume
        .upload_part(absent, upload.payload(), 1, Bytes::from_static(b"whole"))
        .await
        .unwrap();
    let created = volume
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
    assert_read(volume, "new-large", &created, None, b"whole").await;

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

    let empty_slash = WriteTarget::File("multipart-marker/");
    let upload = volume.start_multipart(empty_slash).await.unwrap();
    let part = volume
        .upload_part(empty_slash, upload.payload(), 1, Bytes::new())
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(
                empty_slash,
                upload.payload(),
                &[part],
                WriteCondition::Absent
            )
            .await,
        Err(VolumeError::InvalidRequest)
    );
    volume
        .abort_multipart(empty_slash, upload.payload())
        .await
        .unwrap();
    assert_eq!(
        volume.stat("multipart-marker/", None).await,
        Err(VolumeError::NotFound)
    );
    stream::iter([
        ("multipart-marker", "multipart-marker/", Bytes::new()),
        (
            "multipart-slash",
            "multipart-slash/",
            Bytes::from_static(b"slash-part-a7Z!"),
        ),
    ])
    .then(|(bare, slash, body)| async move {
        let target = WriteTarget::File(slash);
        let upload = volume.start_multipart(target).await.unwrap();
        let mut part = volume
            .upload_part(target, upload.payload(), 1, body.clone())
            .await
            .unwrap();
        assert_eq!(part.size, body.len() as u64);
        if body.is_empty() {
            part.size = 1;
        }
        let reference = volume
            .complete_multipart(
                target,
                upload.payload(),
                std::slice::from_ref(&part),
                WriteCondition::Absent,
            )
            .await
            .unwrap();
        let metadata = volume.stat(bare, None).await.unwrap();
        assert_eq!(metadata.key.as_ref(), bare);
        assert_eq!(metadata.size, body.len() as u64);
        assert_eq!(metadata.version, reference);
        assert_read(volume, bare, &reference, None, &body).await;
        assert_read(
            volume,
            bare,
            &reference,
            Some(body.len() as u64..=u64::MAX),
            b"",
        )
        .await;
        assert_eq!(volume.stat(slash, None).await, Err(VolumeError::NotFound));
        stream::iter([None, Some(0..=0)])
            .then(|range| {
                let reference = &reference;
                async move {
                    assert_eq!(
                        volume.read(slash, reference, range).await.unwrap_err(),
                        VolumeError::NotFound
                    );
                }
            })
            .collect::<()>()
            .await;
        let expected = [ListedObject {
            key: bare.into(),
            size: body.len() as u64,
            etag: reference.etag.clone(),
        }];
        let page = volume.list(bare, None).await.unwrap();
        assert!(page.continuation.is_none());
        assert_eq!(page.objects.as_ref(), expected);

        let rejected = volume.start_multipart(target).await.unwrap();
        let rejected_body = if body.is_empty() {
            Bytes::new()
        } else {
            Bytes::from_static(b"must-not-replace")
        };
        let mut part = volume
            .upload_part(target, rejected.payload(), 1, rejected_body)
            .await
            .unwrap();
        if body.is_empty() {
            part.size = 1;
        }
        assert_eq!(
            volume
                .complete_multipart(target, rejected.payload(), &[part], WriteCondition::Absent)
                .await,
            Err(VolumeError::Exists)
        );
        volume
            .abort_multipart(target, rejected.payload())
            .await
            .unwrap();
        assert_eq!(volume.stat(bare, None).await.unwrap(), metadata);
        assert_read(volume, bare, &reference, None, &body).await;
        let page = volume.list(bare, None).await.unwrap();
        assert!(page.continuation.is_none());
        assert_eq!(page.objects.as_ref(), expected);
        assert_eq!(volume.stat(slash, None).await, Err(VolumeError::NotFound));
    })
    .collect::<()>()
    .await;
    let metadata = volume.stat("multipart-document", None).await.unwrap();
    assert_eq!(metadata.size, 17);
    volume.delete(key, &metadata).await.unwrap();
    assert_eq!(
        volume.stat("multipart-document", None).await,
        Err(VolumeError::NotFound)
    );
}

async fn multipart_part_identity_and_sizes(volume: &dyn VolumeClient, versioned: bool) {
    let target = WriteTarget::File("multipart-validation");
    let upload = volume.start_multipart(target).await.unwrap();
    let first = volume
        .upload_part(target, upload.payload(), 1, Bytes::from_static(b"short"))
        .await
        .unwrap();
    let mut wrong = first.clone();
    wrong.etag = "\"not-the-uploaded-part\"".into();
    assert_eq!(
        volume
            .complete_multipart(target, upload.payload(), &[wrong], WriteCondition::Absent)
            .await,
        Err(VolumeError::RequestFailed)
    );
    let second = volume
        .upload_part(target, upload.payload(), 2, Bytes::from_static(b"last"))
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(
                target,
                upload.payload(),
                &[first, second.clone()],
                WriteCondition::Absent,
            )
            .await,
        Err(VolumeError::RequestFailed)
    );
    assert_eq!(
        volume.stat("multipart-validation", None).await,
        Err(VolumeError::NotFound)
    );
    let size = 5 * 1024 * 1024;
    let mut first = volume
        .upload_part(target, upload.payload(), 1, Bytes::from(vec![b'a'; size]))
        .await
        .unwrap();
    first.size += 1;
    let parts = [first, second];
    let reference = volume
        .complete_multipart(target, upload.payload(), &parts, WriteCondition::Absent)
        .await
        .unwrap();
    assert_eq!(
        volume
            .stat("multipart-validation", None)
            .await
            .unwrap()
            .size,
        size as u64 + 4
    );
    assert_read(
        volume,
        "multipart-validation",
        &reference,
        Some(size as u64 - 2..=u64::MAX),
        b"aalast",
    )
    .await;
    assert_eq!(
        volume
            .complete_multipart(target, upload.payload(), &parts, WriteCondition::Absent)
            .await,
        Err(VolumeError::Exists)
    );
    assert_eq!(
        volume
            .complete_multipart(
                target,
                upload.payload(),
                &parts,
                WriteCondition::Matches(&reference.etag),
            )
            .await,
        Ok(reference.clone())
    );
    let mut wrong = parts.clone();
    wrong[1].etag = "\"not-the-completed-part\"".into();
    assert_eq!(
        volume
            .complete_multipart(
                target,
                upload.payload(),
                &wrong,
                WriteCondition::Matches(&reference.etag),
            )
            .await,
        Err(VolumeError::RequestFailed)
    );
    assert_eq!(
        volume
            .complete_multipart(
                target,
                upload.payload(),
                &parts[..1],
                WriteCondition::Matches(&reference.etag),
            )
            .await,
        Err(VolumeError::RequestFailed)
    );
    assert_eq!(
        volume
            .complete_multipart(
                target,
                upload.payload(),
                &parts,
                WriteCondition::Matches(&reference.etag),
            )
            .await,
        Ok(reference.clone())
    );
    assert_eq!(
        volume
            .stat("multipart-validation", None)
            .await
            .unwrap()
            .version,
        reference
    );
    let replacement = volume
        .put(
            target,
            WriteCondition::Matches(&reference.etag),
            Bytes::from_static(b"replacement-9qX"),
        )
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(
                target,
                upload.payload(),
                &parts,
                WriteCondition::Matches(&reference.etag),
            )
            .await,
        Err(VolumeError::Conflict)
    );
    assert_eq!(
        volume
            .complete_multipart(
                target,
                upload.payload(),
                &parts,
                WriteCondition::Matches(&replacement.etag),
            )
            .await,
        if versioned {
            Ok(reference)
        } else {
            Err(VolumeError::NotFound)
        }
    );
    assert_eq!(
        volume
            .stat("multipart-validation", None)
            .await
            .unwrap()
            .version,
        replacement
    );
    assert_read(
        volume,
        "multipart-validation",
        &replacement,
        None,
        b"replacement-9qX",
    )
    .await;
}

async fn probes_and_invalid_paths(volume: &dyn VolumeClient, versioned: bool) {
    let probe = volume
        .put(
            WriteTarget::Probe("probe"),
            WriteCondition::Absent,
            Bytes::from_static(b"probe-content"),
        )
        .await
        .unwrap();
    let metadata = volume.stat(".golem/probe", None).await.unwrap();
    assert_eq!(metadata.key.as_ref(), ".golem/probe");
    assert_eq!(metadata.size, 13);
    assert_eq!(metadata.version, probe);
    assert_read(volume, ".golem/probe", &probe, None, b"probe-content").await;
    assert_eq!(
        volume.list(".golem/", None).await.unwrap().objects.as_ref(),
        [ListedObject {
            key: ".golem/probe".into(),
            size: 13,
            etag: probe.etag.clone(),
        }]
    );
    stream::iter([".golem", ".golem/probe"])
        .then(|key| async move {
            assert_eq!(
                volume
                    .put(
                        WriteTarget::File(key),
                        WriteCondition::Absent,
                        Bytes::from_static(b"reserved")
                    )
                    .await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume.start_multipart(WriteTarget::File(key)).await,
                Err(VolumeError::InvalidRequest)
            );
        })
        .collect::<()>()
        .await;
    stream::iter([
        "/absolute",
        "../parent",
        "nested/../parent",
        "./dot",
        "nested/./dot",
    ])
    .then(|key| {
        let probe = &probe;
        async move {
            assert_eq!(
                volume.stat(key, None).await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume.read(key, probe, None).await.unwrap_err(),
                VolumeError::InvalidRequest
            );
            assert_eq!(
                volume.list(key, None).await.unwrap_err(),
                VolumeError::InvalidRequest
            );
            assert_eq!(
                volume
                    .put(
                        WriteTarget::File(key),
                        WriteCondition::Absent,
                        Bytes::from_static(b"invalid")
                    )
                    .await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume
                    .put(
                        WriteTarget::Probe(key),
                        WriteCondition::Absent,
                        Bytes::from_static(b"invalid")
                    )
                    .await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume.start_multipart(WriteTarget::File(key)).await,
                Err(VolumeError::InvalidRequest)
            );
        }
    })
    .collect::<()>()
    .await;
    assert_eq!(
        volume
            .put(
                WriteTarget::File("marker/"),
                WriteCondition::Absent,
                Bytes::new()
            )
            .await,
        Err(VolumeError::InvalidRequest)
    );
    assert_eq!(
        volume
            .delete(WriteTarget::File(".golem/probe"), &metadata)
            .await,
        Err(VolumeError::InvalidRequest)
    );
    assert_eq!(
        volume
            .delete(WriteTarget::File("not-probe"), &metadata)
            .await,
        Err(VolumeError::InvalidRequest)
    );
    let marker = volume
        .delete(WriteTarget::Probe("probe"), &metadata)
        .await
        .unwrap();
    assert_eq!(marker.is_some(), versioned);
    assert_eq!(
        volume.stat(".golem/probe", None).await,
        Err(VolumeError::NotFound)
    );
}

async fn key_byte_contract(volume: &dyn VolumeClient, key_bytes: usize) {
    stream::iter([false, true])
        .flat_map(|is_probe| {
            let name_bytes = key_bytes - if is_probe { 7 } else { 0 };
            stream::iter([
                "a".repeat(name_bytes),
                format!(
                    "{}{}",
                    "é".repeat(name_bytes / 2),
                    "a".repeat(name_bytes % 2)
                ),
            ])
            .map(move |name| (is_probe, name))
        })
        .then(|(is_probe, name)| async move {
            let key = if is_probe {
                format!(".golem/{name}").into_boxed_str()
            } else {
                name.into_boxed_str()
            };
            let invalid = format!("{key}x").into_boxed_str();
            assert_eq!(key.len(), key_bytes);
            assert_eq!(invalid.len(), key_bytes + 1);
            let target = if is_probe {
                WriteTarget::Probe(&key[7..])
            } else {
                WriteTarget::File(&key)
            };
            let first = volume
                .put(
                    target,
                    WriteCondition::Absent,
                    Bytes::from_static(b"root-seed-A7"),
                )
                .await
                .unwrap();
            let mut metadata = volume.stat(&key, None).await.unwrap();
            assert_eq!(metadata.key.as_ref(), key.as_ref());
            assert_eq!(metadata.size, 12);
            assert_eq!(metadata.version, first);
            assert_read(volume, &key, &first, None, b"root-seed-A7").await;
            let upload = volume.start_multipart(target).await.unwrap();
            let part = volume
                .upload_part(
                    target,
                    upload.payload(),
                    1,
                    Bytes::from_static(b"root-publish-Q9"),
                )
                .await
                .unwrap();
            let invalid_target = if is_probe {
                WriteTarget::Probe(&invalid[7..])
            } else {
                WriteTarget::File(&invalid)
            };
            assert_eq!(
                volume.stat(&invalid, None).await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume.read(&invalid, &first, None).await.unwrap_err(),
                VolumeError::InvalidRequest
            );
            assert_eq!(
                volume
                    .put(
                        invalid_target,
                        WriteCondition::Absent,
                        Bytes::from_static(b"invalid")
                    )
                    .await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume.start_multipart(invalid_target).await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume
                    .upload_part(
                        invalid_target,
                        upload.payload(),
                        1,
                        Bytes::from_static(b"invalid")
                    )
                    .await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume
                    .complete_multipart(
                        invalid_target,
                        upload.payload(),
                        std::slice::from_ref(&part),
                        WriteCondition::Absent
                    )
                    .await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume
                    .abort_multipart(invalid_target, upload.payload())
                    .await,
                Err(VolumeError::InvalidRequest)
            );
            metadata.key = invalid.as_ref().into();
            assert_eq!(
                volume.delete(invalid_target, &metadata).await,
                Err(VolumeError::InvalidRequest)
            );
            let page = volume.list(&invalid, None).await.unwrap();
            assert!(page.objects.is_empty());
            assert!(page.continuation.is_none());
            let unchanged = volume.stat(&key, None).await.unwrap();
            assert_eq!(unchanged.key.as_ref(), key.as_ref());
            assert_eq!(unchanged.version, first);
            assert_eq!(unchanged.size, 12);
            assert_read(volume, &key, &first, None, b"root-seed-A7").await;
            let published = volume
                .complete_multipart(
                    target,
                    upload.payload(),
                    &[part],
                    WriteCondition::Matches(&first.etag),
                )
                .await
                .unwrap();
            let metadata = volume.stat(&key, None).await.unwrap();
            assert_eq!(metadata.key.as_ref(), key.as_ref());
            assert_eq!(metadata.size, 15);
            assert_eq!(metadata.version, published);
            assert_read(volume, &key, &published, None, b"root-publish-Q9").await;
            let page = volume.list(&key, None).await.unwrap();
            assert!(page.continuation.is_none());
            assert_eq!(
                page.objects.as_ref(),
                [ListedObject {
                    key: key.as_ref().into(),
                    size: 15,
                    etag: published.etag
                }]
            );
            volume.delete(target, &metadata).await.unwrap();
            assert_eq!(volume.stat(&key, None).await, Err(VolumeError::NotFound));
            let aborted = volume.start_multipart(target).await.unwrap();
            let part = volume
                .upload_part(
                    target,
                    aborted.payload(),
                    1,
                    Bytes::from_static(b"aborted-part"),
                )
                .await
                .unwrap();
            volume
                .abort_multipart(target, aborted.payload())
                .await
                .unwrap();
            assert_eq!(
                volume
                    .complete_multipart(target, aborted.payload(), &[part], WriteCondition::Absent)
                    .await,
                Err(VolumeError::NotFound)
            );
            assert_eq!(volume.stat(&key, None).await, Err(VolumeError::NotFound));
        })
        .collect::<()>()
        .await;
}

async fn empty_key_contract(volume: &dyn VolumeClient, expected: VolumeError) {
    let reference = ObjectVersion {
        etag: "\"missing-reference\"".into(),
        version_id: None,
    };
    assert_eq!(volume.stat("", None).await, Err(expected));
    assert_eq!(
        volume.read("", &reference, None).await.unwrap_err(),
        expected
    );
    assert_eq!(
        volume
            .read("", &reference, Some(RangeInclusive::new(2, 1)))
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    let invalid = ObjectVersion {
        etag: "not-a-strong-etag".into(),
        version_id: None,
    };
    assert_eq!(
        volume.read("", &invalid, None).await.unwrap_err(),
        VolumeError::InvalidRequest
    );
    let page = volume.list(&"x".repeat(1025), None).await.unwrap();
    assert!(page.objects.is_empty());
    assert!(page.continuation.is_none());
}

fn empty_key_prefixes() -> [(Box<str>, VolumeError); 5] {
    [
        ("".into(), VolumeError::NotFound),
        (
            format!("{}/", "p".repeat(1023)).into_boxed_str(),
            VolumeError::NotFound,
        ),
        (
            format!("{}/", "p".repeat(1024)).into_boxed_str(),
            VolumeError::InvalidRequest,
        ),
        (
            format!("{}a/", "é".repeat(511)).into_boxed_str(),
            VolumeError::NotFound,
        ),
        (
            format!("{}/", "é".repeat(512)).into_boxed_str(),
            VolumeError::InvalidRequest,
        ),
    ]
}

async fn memory_contract(versioned: bool) {
    stream::iter([("", 1024), ("corpus/", 1017), ("tenant/é/", 1014)])
        .then(|(prefix, bytes)| async move {
            let volume: Arc<dyn VolumeClient> =
                Arc::new(InMemoryVolume::new(versioned, 1000, prefix).unwrap());
            if prefix == "corpus/" {
                volume_contract(volume.as_ref(), versioned).await;
            }
            key_byte_contract(volume.as_ref(), bytes).await;
            empty_key_contract(volume.as_ref(), VolumeError::NotFound).await;
        })
        .collect::<()>()
        .await;
    stream::iter(empty_key_prefixes())
        .then(|(prefix, expected)| async move {
            let volume = InMemoryVolume::new(versioned, 1000, &prefix).unwrap();
            empty_key_contract(&volume, expected).await;
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn memory_versioned_volume_contract() {
    memory_contract(true).await;
}

#[test]
#[timeout("30s")]
async fn memory_unversioned_volume_contract() {
    memory_contract(false).await;
}

async fn rustfs_contract(versioned: bool) {
    rustfs_case(versioned, async |server, setup, bucket| {
        let client: Arc<dyn VolumeClient> = Arc::new(volume(server, bucket, "corpus/", None));
        volume_contract(client.as_ref(), versioned).await;
        stream::iter([("", 1024), ("corpus/", 1017), ("tenant/é/", 1014)])
            .then(|(prefix, bytes)| async move {
                let client = volume(server, bucket, prefix, None);
                key_byte_contract(&client, bytes).await;
                empty_key_contract(&client, VolumeError::NotFound).await;
            })
            .collect::<()>()
            .await;
        stream::iter(empty_key_prefixes())
            .then(|(prefix, expected)| async move {
                let client = volume(server, bucket, &prefix, None);
                empty_key_contract(&client, expected).await;
            })
            .collect::<()>()
            .await;
        let uploads = setup
            .list_multipart_uploads()
            .bucket(bucket)
            .send()
            .await
            .unwrap();
        assert!(uploads.uploads().is_empty());
    })
    .await;
}

#[test]
async fn rustfs_versioned_volume_contract() {
    rustfs_contract(true).await;
}

#[test]
async fn rustfs_unversioned_volume_contract() {
    rustfs_contract(false).await;
}

#[test]
async fn rustfs_lists_foreign_files_but_hides_empty_markers() {
    rustfs_case(true, async |server, setup, bucket| {
        let volume = volume(server, bucket, "corpus/", None);
        let expected = stream::iter([
            ("corpus/", Bytes::new(), None),
            ("corpus/dir/", Bytes::new(), None),
            ("outside-prefix", Bytes::from_static(b"not-listed"), None),
            (
                "corpus/.golem/foreign",
                Bytes::from_static(b"reserved-for-probes"),
                Some(".golem/foreign"),
            ),
            ("corpus/dir/file", Bytes::new(), Some("dir/file")),
            (
                "corpus/dir/nonempty/",
                Bytes::from_static(b"not-a-marker"),
                Some("dir/nonempty/"),
            ),
            (
                "corpus/ordinary",
                Bytes::from_static(b"visible"),
                Some("ordinary"),
            ),
        ])
        .then(|(key, body, visible_key)| async move {
            let size = body.len() as u64;
            let published = setup
                .put_object()
                .bucket(bucket)
                .key(key)
                .body(ByteStream::from(body))
                .send()
                .await
                .unwrap();
            visible_key.map(|key| ListedObject {
                key: key.into(),
                size,
                etag: published.e_tag().unwrap().into(),
            })
        })
        .filter_map(futures::future::ready)
        .collect::<Vec<_>>()
        .await
        .into_boxed_slice();
        let page = volume.list("", None).await.unwrap();
        assert_eq!(page.objects, expected);
        assert!(page.continuation.is_none());
        assert_eq!(volume.stat("", None).await, Err(VolumeError::NotFound));
        assert_eq!(volume.stat("dir/", None).await, Err(VolumeError::NotFound));
        let empty = volume.stat("dir/file", None).await.unwrap();
        assert_eq!(empty.size, 0);
        assert_read(&volume, "dir/file", &empty.version, None, b"").await;
        let slash = volume.stat("dir/nonempty/", None).await.unwrap();
        assert_read(
            &volume,
            "dir/nonempty/",
            &slash.version,
            None,
            b"not-a-marker",
        )
        .await;
        let reserved = volume.stat(".golem/foreign", None).await.unwrap();
        assert_read(
            &volume,
            ".golem/foreign",
            &reserved.version,
            None,
            b"reserved-for-probes",
        )
        .await;
    })
    .await;
}

async fn encrypted_roundtrips(
    encryption: Encryption,
    algorithm: ServerSideEncryption,
    kms: Option<&str>,
) {
    rustfs_case(true, async move |server, setup, bucket| {
        let volume = volume(server, bucket, "corpus/", Some(encryption));
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
            let algorithm = &algorithm;
            async move {
                let head = setup
                    .head_object()
                    .bucket(bucket)
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
                let read = volume.read(key, &published, None).await.unwrap();
                assert_eq!(read.version, published);
                assert_eq!(read.body.collect().await.unwrap(), body);
            }
        })
        .collect::<()>()
        .await;
    })
    .await;
}

#[test]
async fn rustfs_sse_s3_preserves_single_and_multipart_objects() {
    encrypted_roundtrips(Encryption::S3, ServerSideEncryption::Aes256, None).await;
}

#[test]
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
async fn rustfs_sse_kms_rejects_a_missing_key_without_publishing() {
    rustfs_case(true, async |server, setup, bucket| {
        let volume = volume(
            server,
            bucket,
            "corpus/",
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
            .bucket(bucket)
            .key("corpus/missing-kms")
            .send()
            .await
            .unwrap_err();
        assert_eq!(head.raw_response().unwrap().status().as_u16(), 404);
    })
    .await;
}
