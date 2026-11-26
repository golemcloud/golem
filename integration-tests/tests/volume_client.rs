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
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{BucketVersioningStatus, VersioningConfiguration};
use bytes::Bytes;
use futures::{FutureExt, StreamExt, TryStreamExt, stream};
use golem_common::model::volume::{Entry, FileState, Node, VolumeError, VolumePath};
use golem_service_base::aws::AwsCredentials;
use golem_service_base::volume::connection::ConnectionPolicy;
use golem_service_base::volume::memory::{InMemoryVolume, MemoryControl};
use golem_service_base::volume::s3::{S3VolumeConfig, connect, place};
use golem_service_base::volume::{VolumeClient, VolumeFile, VolumeRecovery};
use golem_test_framework::components::s3::{DockerRustFs, S3Server};
use std::ops::Range;
use std::panic::{AssertUnwindSafe, resume_unwind};
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

fn config(server: &DockerRustFs, bucket: &str, prefix: &str) -> S3VolumeConfig {
    S3VolumeConfig {
        endpoint: Some(server.endpoint().into_boxed_str()),
        region: "us-east-1".into(),
        bucket: bucket.into(),
        prefix: prefix.into(),
    }
}

fn credentials(server: &DockerRustFs) -> AwsCredentials {
    AwsCredentials::from_record(&serde_json::json!({
        "accessKeyId": server.access_key_id(), "secretAccessKey": server.secret_access_key()
    }))
    .unwrap()
}

fn policy() -> ConnectionPolicy {
    ConnectionPolicy {
        allow_loopback: true,
        ..ConnectionPolicy::new(Box::new([]))
    }
}

// These controls act as a competing writer, not as part of the read interface.
enum Control<'a> {
    Memory(MemoryControl),
    S3 {
        setup: &'a aws_sdk_s3::Client,
        bucket: &'a str,
        prefix: &'a str,
    },
}

impl Control<'_> {
    async fn write(&self, path: &VolumePath, bytes: Bytes) {
        match self {
            Self::Memory(control) => {
                control.write(path, bytes).unwrap();
            }
            Self::S3 {
                setup,
                bucket,
                prefix,
            } => {
                setup
                    .put_object()
                    .bucket(*bucket)
                    .key(format!("{prefix}{}", path.as_str()))
                    .body(ByteStream::from(bytes))
                    .send()
                    .await
                    .unwrap();
            }
        }
    }

    async fn forget(&self, path: &VolumePath, state: &FileState) {
        match self {
            Self::Memory(control) => control.forget(path, state),
            Self::S3 {
                setup,
                bucket,
                prefix,
            } => {
                setup
                    .delete_object()
                    .bucket(*bucket)
                    .key(format!("{prefix}{}", path.as_str()))
                    .set_version_id(state.reference().1.map(str::to_owned))
                    .send()
                    .await
                    .unwrap();
            }
        }
    }
}

fn path(value: &str) -> VolumePath {
    VolumePath::new(value).unwrap()
}

fn entry(value: &str, node: Node) -> Entry {
    Entry {
        path: path(value),
        node,
    }
}

async fn entries(volume: &dyn VolumeClient, directory: &VolumePath) -> Box<[Entry]> {
    let mut entries = stream::try_unfold(Some(None), |after| async move {
        let Some(after) = after else { return Ok(None) };
        let page = volume.list(directory, after.as_ref()).await?;
        Ok::<_, VolumeError>(Some((page.entries, page.after.map(Some))))
    })
    .try_fold(Vec::new(), |mut entries, page| async move {
        entries.extend(page);
        Ok(entries)
    })
    .await
    .unwrap();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries.into_boxed_slice()
}

async fn read(file: &VolumeFile, range: Range<u64>) -> Result<Bytes, VolumeError> {
    file.read(range)
        .await?
        .try_fold(Vec::new(), |mut bytes, chunk| async move {
            bytes.extend_from_slice(&chunk);
            Ok(bytes)
        })
        .await
        .map(Bytes::from)
}

async fn stat_and_direct_children(volume: &dyn VolumeClient, control: &Control<'_>) {
    stream::iter([
        ("catalog/empty", Bytes::new()),
        ("catalog/file", Bytes::from_static(b"listed bytes")),
        ("catalog/nested/first", Bytes::from_static(b"first")),
        ("catalog/nested/second", Bytes::from_static(b"second")),
        ("catalogue/outside", Bytes::from_static(b"not a child")),
    ])
    .then(|(name, bytes)| async move { control.write(&path(name), bytes).await })
    .collect::<()>()
    .await;

    assert_eq!(volume.stat(&path("")).await.unwrap(), Some(Node::Directory));
    assert_eq!(
        volume.stat(&path("catalog")).await.unwrap(),
        Some(Node::Directory)
    );
    assert_eq!(
        volume.stat(&path("catalog/nested")).await.unwrap(),
        Some(Node::Directory)
    );
    assert_eq!(
        volume.stat(&path("catalog/file")).await.unwrap(),
        Some(Node::File { size: 12 })
    );
    assert_eq!(
        volume.stat(&path("catalog/empty")).await.unwrap(),
        Some(Node::File { size: 0 })
    );
    assert_eq!(
        entries(volume, &path("")).await.as_ref(),
        [
            entry("catalog", Node::Directory),
            entry("catalogue", Node::Directory),
        ]
    );
    assert_eq!(
        entries(volume, &path("catalog")).await.as_ref(),
        [
            entry("catalog/empty", Node::File { size: 0 }),
            entry("catalog/file", Node::File { size: 12 }),
            entry("catalog/nested", Node::Directory),
        ]
    );
    assert_eq!(
        entries(volume, &path("catalog/nested")).await.as_ref(),
        [
            entry("catalog/nested/first", Node::File { size: 5 }),
            entry("catalog/nested/second", Node::File { size: 6 }),
        ]
    );
    assert_eq!(volume.stat(&path("missing")).await.unwrap(), None);
    assert_eq!(
        volume.open(&path("missing")).await.unwrap_err(),
        VolumeError::NotFound
    );
}

async fn open_ranges_and_empty_files(
    volume: &dyn VolumeClient,
    control: &Control<'_>,
    retained: bool,
) {
    control
        .write(&path("ranges"), Bytes::from_static(b"0123456789abcdef"))
        .await;
    let file = volume.open(&path("ranges")).await.unwrap();
    assert_eq!(file.size(), 16);
    assert_eq!(file.replayable(), retained);
    stream::iter([
        (0..16, b"0123456789abcdef".as_slice()),
        (3..7, b"3456".as_slice()),
        (14..20, b"ef".as_slice()),
        (5..5, b"".as_slice()),
        (16..17, b"".as_slice()),
        (19..u64::MAX, b"".as_slice()),
    ])
    .then(|(range, expected)| {
        let file = &file;
        async move { assert_eq!(read(file, range).await.unwrap().as_ref(), expected) }
    })
    .collect::<()>()
    .await;

    control.write(&path("empty"), Bytes::new()).await;
    let empty = volume.open(&path("empty")).await.unwrap();
    assert_eq!(empty.size(), 0);
    assert_eq!(empty.replayable(), retained);
    assert!(read(&empty, 0..0).await.unwrap().is_empty());
    assert!(read(&empty, 0..1).await.unwrap().is_empty());
}

async fn recorded_state_after_replacement(
    volume: &dyn VolumeClient,
    recovery: &dyn VolumeRecovery,
    control: &Control<'_>,
    retained: bool,
) {
    let name = path("replaced");
    control
        .write(&name, Bytes::from_static(b"original bytes"))
        .await;
    let original = volume.open(&name).await.unwrap();
    let recorded = original.state().clone();
    let reopened = recovery.open_recorded(&name, &recorded).await.unwrap();
    assert_eq!(reopened.state(), &recorded);
    assert_eq!(
        read(&reopened, 0..14).await.unwrap().as_ref(),
        b"original bytes"
    );

    control
        .write(&name, Bytes::from_static(b"replacement"))
        .await;
    let current = volume.open(&name).await.unwrap();
    assert_eq!(current.size(), 11);
    assert_ne!(current.state(), &recorded);
    assert_eq!(
        read(&current, 0..11).await.unwrap().as_ref(),
        b"replacement"
    );

    if retained {
        let recovered = recovery.open_recorded(&name, &recorded).await.unwrap();
        assert_eq!(recovered.state(), &recorded);
        assert_eq!(recovered.size(), 14);
        assert!(recovered.replayable());
        assert_eq!(
            read(&recovered, 0..14).await.unwrap().as_ref(),
            b"original bytes"
        );
        assert_eq!(read(&original, 3..8).await.unwrap().as_ref(), b"ginal");
        assert_eq!(
            read(&original, 0..14).await.unwrap().as_ref(),
            b"original bytes"
        );

        control.forget(&name, &recorded).await;
        assert_eq!(
            recovery.open_recorded(&name, &recorded).await.unwrap_err(),
            VolumeError::Changed
        );
        assert_eq!(read(&original, 0..14).await, Err(VolumeError::Changed));
        assert_eq!(read(&recovered, 0..14).await, Err(VolumeError::Changed));
        assert_eq!(
            read(&current, 0..11).await.unwrap().as_ref(),
            b"replacement"
        );
    } else {
        assert_eq!(
            recovery.open_recorded(&name, &recorded).await.unwrap_err(),
            VolumeError::Changed
        );
        assert_eq!(read(&original, 0..14).await, Err(VolumeError::Changed));
        assert_eq!(read(&reopened, 0..14).await, Err(VolumeError::Changed));
    }
}

async fn selected_state_disappears(
    volume: &dyn VolumeClient,
    recovery: &dyn VolumeRecovery,
    control: &Control<'_>,
) {
    let name = path("forgotten");
    control
        .write(&name, Bytes::from_static(b"selected state"))
        .await;
    let file = volume.open(&name).await.unwrap();
    control.forget(&name, file.state()).await;
    assert_eq!(volume.stat(&name).await.unwrap(), None);
    assert_eq!(volume.open(&name).await.unwrap_err(), VolumeError::NotFound);
    assert_eq!(
        recovery
            .open_recorded(&name, file.state())
            .await
            .unwrap_err(),
        VolumeError::Changed
    );
    assert_eq!(read(&file, 0..14).await, Err(VolumeError::Changed));
}

async fn read_contract(
    volume: &dyn VolumeClient,
    recovery: &dyn VolumeRecovery,
    control: &Control<'_>,
    retained: bool,
) {
    stat_and_direct_children(volume, control).await;
    open_ranges_and_empty_files(volume, control, retained).await;
    recorded_state_after_replacement(volume, recovery, control, retained).await;
    selected_state_disappears(volume, recovery, control).await;
}

async fn memory_contract(retained: bool) {
    let (volume, control) = InMemoryVolume::new(retained, 2).unwrap();
    read_contract(&volume, &volume, &Control::Memory(control), retained).await;
}

#[test]
#[timeout("30s")]
async fn memory_retained_read_contract() {
    memory_contract(true).await;
}

#[test]
#[timeout("30s")]
async fn memory_unretained_read_contract() {
    memory_contract(false).await;
}

async fn rustfs_contract(retained: bool) {
    rustfs_case(retained, async |server, setup, bucket| {
        let (volume, recovery) = connect(
            config(server, bucket, "corpus/"),
            credentials(server),
            policy(),
        )
        .unwrap();
        let control = Control::S3 {
            setup,
            bucket,
            prefix: "corpus/",
        };
        read_contract(volume.as_ref(), recovery.as_ref(), &control, retained).await;
    })
    .await;
}

#[test]
async fn rustfs_retained_read_contract() {
    rustfs_contract(true).await;
}

#[test]
async fn rustfs_unretained_read_contract() {
    rustfs_contract(false).await;
}

#[test]
async fn rustfs_hides_foreign_folder_markers_without_phantom_directories() {
    rustfs_case(true, async |server, setup, bucket| {
        stream::iter([
            ("corpus/", Bytes::new()),
            ("corpus/dir/", Bytes::new()),
            ("corpus/ghost/", Bytes::new()),
            ("corpus/hollow/deep/", Bytes::new()),
            ("corpus/dir/file", Bytes::new()),
            ("corpus/ordinary", Bytes::from_static(b"visible")),
            ("outside-prefix", Bytes::from_static(b"not listed")),
        ])
        .then(|(key, bytes)| async move {
            setup
                .put_object()
                .bucket(bucket)
                .key(key)
                .body(ByteStream::from(bytes))
                .send()
                .await
                .unwrap();
        })
        .collect::<()>()
        .await;
        let (volume, _) = connect(
            config(server, bucket, "corpus/"),
            credentials(server),
            policy(),
        )
        .unwrap();
        assert_eq!(
            entries(volume.as_ref(), &path("")).await.as_ref(),
            [
                entry("dir", Node::Directory),
                entry("ordinary", Node::File { size: 7 }),
            ]
        );
        assert_eq!(
            entries(volume.as_ref(), &path("dir")).await.as_ref(),
            [entry("dir/file", Node::File { size: 0 })]
        );
        assert_eq!(volume.stat(&path("ghost")).await.unwrap(), None);
        assert_eq!(volume.stat(&path("hollow")).await.unwrap(), None);
        assert_eq!(
            volume.stat(&path("dir")).await.unwrap(),
            Some(Node::Directory)
        );
        assert!(entries(volume.as_ref(), &path("ghost")).await.is_empty());
        assert!(entries(volume.as_ref(), &path("hollow")).await.is_empty());
    })
    .await;
}

#[test]
async fn rustfs_placement_refuses_a_bucket_without_versioning() {
    rustfs_case(false, async |server, _, bucket| {
        let error = place(
            config(server, bucket, "corpus/"),
            credentials(server),
            policy(),
        )
        .await
        .unwrap_err();
        let VolumeError::Requirement { requirement } = error else {
            panic!("expected a versioning requirement, got {error:?}");
        };
        assert!(requirement.contains("versioning"));
    })
    .await;
}
