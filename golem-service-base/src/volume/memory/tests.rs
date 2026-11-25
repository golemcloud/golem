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

use super::InMemoryVolume;
use crate::volume::{
    BucketVersioning, ListedObject, ObjectMetadata, ObjectVersion, UploadedPart, VolumeClient,
    VolumeError, WriteCondition, WriteTarget,
};
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;
use test_r::test;

fn client(versioned: bool, page_size: usize) -> Arc<dyn VolumeClient> {
    Arc::new(InMemoryVolume::new(versioned, page_size, "").unwrap())
}

#[test]
fn invalid_page_size_or_prefix_is_rejected() {
    [(0, ""), (2, "/"), (2, "corpus"), (2, "./"), (2, "a/../")]
        .into_iter()
        .for_each(|(page_size, prefix)| {
            assert_eq!(
                InMemoryVolume::new(false, page_size, prefix).unwrap_err(),
                VolumeError::InvalidRequest
            );
        });
}

#[test]
async fn every_object_operation_rejects_full_keys_over_1024_utf8_bytes() {
    use futures::stream;
    stream::iter([("", 1024), ("corpus/", 1017), ("tenant/é/", 1014)])
        .flat_map(|(prefix, bytes)| {
            stream::iter([false, true]).map(move |is_probe| (prefix, bytes, is_probe))
        })
        .then(|(prefix, bytes, is_probe)| async move {
            let volume = InMemoryVolume::new(false, 2, prefix).unwrap();
            let name_bytes = bytes - if is_probe { 7 } else { 0 };
            let name = format!(
                "{}{}",
                "é".repeat(name_bytes / 2),
                "x".repeat(name_bytes % 2)
            );
            let target = if is_probe {
                WriteTarget::Probe(&name)
            } else {
                WriteTarget::File(&name)
            };
            let created = volume
                .put(target, WriteCondition::Absent, Bytes::from_static(b"abc"))
                .await
                .unwrap();
            let relative = if is_probe {
                format!(".golem/{name}")
            } else {
                name.clone()
            };
            assert_eq!(volume.stat(&relative, None).await.unwrap().version, created);
            assert_eq!(
                volume
                    .read(&relative, &created, None)
                    .await
                    .unwrap()
                    .body
                    .collect()
                    .await
                    .unwrap(),
                "abc"
            );
            let name = format!("{name}x");
            let target = if is_probe {
                WriteTarget::Probe(&name)
            } else {
                WriteTarget::File(&name)
            };
            let relative = if is_probe {
                format!(".golem/{name}")
            } else {
                name.clone()
            };
            let metadata = ObjectMetadata {
                key: relative.clone().into(),
                size: 3,
                version: created.clone(),
            };
            let parts = [UploadedPart {
                number: 1,
                etag: "\"part\"".into(),
                size: 3,
            }];
            assert_eq!(
                volume.stat(&relative, None).await.unwrap_err(),
                VolumeError::InvalidRequest
            );
            assert_eq!(
                volume.read(&relative, &created, None).await.unwrap_err(),
                VolumeError::InvalidRequest
            );
            assert_eq!(
                volume
                    .put(target, WriteCondition::Absent, Bytes::from_static(b"abc"))
                    .await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume.delete(target, &metadata).await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume.start_multipart(target).await.unwrap_err(),
                VolumeError::InvalidRequest
            );
            assert_eq!(
                volume
                    .upload_part(target, "upload", 1, Bytes::from_static(b"abc"))
                    .await
                    .unwrap_err(),
                VolumeError::InvalidRequest
            );
            assert_eq!(
                volume
                    .complete_multipart(target, "upload", &parts, WriteCondition::Absent)
                    .await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(
                volume.abort_multipart(target, "upload").await,
                Err(VolumeError::InvalidRequest)
            );
            assert!(
                volume
                    .list(&"x".repeat(1025), None)
                    .await
                    .unwrap()
                    .objects
                    .is_empty()
            );
        })
        .collect::<()>()
        .await;
}

#[test]
async fn unversioned_replacement_has_a_new_reference_and_no_pinned_history() {
    let volume = client(false, 2);
    let first = volume
        .put(
            WriteTarget::File("a"),
            WriteCondition::Absent,
            Bytes::from("old"),
        )
        .await
        .unwrap();
    let second = volume
        .put(
            WriteTarget::File("a"),
            WriteCondition::Matches(&first.etag),
            Bytes::from("new"),
        )
        .await
        .unwrap();
    assert_ne!(first.etag, second.etag);
    assert_eq!(first.version_id, None);
    assert_eq!(second.version_id, None);
    assert_eq!(
        volume.read("a", &first, None).await.unwrap_err(),
        VolumeError::Conflict
    );
    assert_eq!(
        volume
            .read("a", &second, None)
            .await
            .unwrap()
            .body
            .collect()
            .await
            .unwrap(),
        Bytes::from("new")
    );
    assert_eq!(
        volume.versioning().await.unwrap(),
        BucketVersioning::Unversioned
    );
    assert!(volume.lifecycle().await.unwrap().is_empty());
}

async fn create(volume: &dyn VolumeClient, key: &str, body: &'static str) -> ObjectVersion {
    volume
        .put(
            WriteTarget::File(key),
            WriteCondition::Absent,
            Bytes::from(body),
        )
        .await
        .unwrap()
}

#[test]
async fn same_content_writes_get_distinct_strong_etags_in_memory() {
    let volume = client(false, 2);
    let first = create(volume.as_ref(), "a", "same").await;
    let second = volume
        .put(
            WriteTarget::File("a"),
            WriteCondition::Matches(&first.etag),
            Bytes::from("same"),
        )
        .await
        .unwrap();
    assert_ne!(first.etag, second.etag);
    assert!(second.etag.starts_with('"') && second.etag.ends_with('"'));
    assert!(!second.etag.starts_with("W/"));
}

#[test]
async fn versioned_delete_hides_current_but_retains_pinned_bodies() {
    async fn assert_marker_selection(volume: &dyn VolumeClient, marker: &str) {
        assert_eq!(
            volume.stat("a", Some(marker)).await,
            Err(VolumeError::Unsupported)
        );
        assert_eq!(
            volume
                .stat("a", Some("7d52c7e1-1d62-4fd7-8575-43ebdcab6b5c"))
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
        futures::stream::iter([None, Some(103..=207)])
            .then(|range| {
                let reference = &reference;
                async move {
                    assert_eq!(
                        volume.read("a", reference, range).await.unwrap_err(),
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
                .read("a", &reference, Some(std::ops::RangeInclusive::new(9, 8)))
                .await
                .unwrap_err(),
            VolumeError::InvalidRequest
        );
        let weak = ObjectVersion {
            etag: "W/\"marker-weak-a7Z!\"".into(),
            version_id: reference.version_id,
        };
        assert_eq!(
            volume.read("a", &weak, None).await.unwrap_err(),
            VolumeError::InvalidRequest
        );
    }

    let volume = client(true, 2);
    let first = create(volume.as_ref(), "a", "old").await;
    let second = volume
        .put(
            WriteTarget::File("a"),
            WriteCondition::Matches(&first.etag),
            Bytes::from("new"),
        )
        .await
        .unwrap();
    assert!(first.version_id.is_some());
    assert_ne!(first.version_id, second.version_id);
    let current = volume.stat("a", None).await.unwrap();
    let marker = volume
        .delete(WriteTarget::File("a"), &current)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        Some(marker.payload().as_ref()),
        second.version_id.as_deref()
    );
    assert_eq!(
        volume.stat("a", None).await.unwrap_err(),
        VolumeError::NotFound
    );
    assert!(volume.list("", None).await.unwrap().objects.is_empty());
    assert_marker_selection(volume.as_ref(), marker.payload()).await;
    assert_eq!(
        volume
            .read("a", &first, None)
            .await
            .unwrap()
            .body
            .collect()
            .await
            .unwrap(),
        Bytes::from("old")
    );
    assert_eq!(
        volume
            .read("a", &second, None)
            .await
            .unwrap()
            .body
            .collect()
            .await
            .unwrap(),
        Bytes::from("new")
    );
    assert_eq!(
        volume
            .stat("a", first.version_id.as_deref())
            .await
            .unwrap()
            .version,
        first
    );
    let third = create(volume.as_ref(), "a", "again").await;
    assert_ne!(third.version_id, second.version_id);
    assert_ne!(Some(marker.payload().as_ref()), third.version_id.as_deref());
    assert_marker_selection(volume.as_ref(), marker.payload()).await;
    assert_eq!(volume.stat("a", None).await.unwrap().version, third);
    assert_eq!(
        volume
            .read("a", &third, None)
            .await
            .unwrap()
            .body
            .collect()
            .await
            .unwrap(),
        Bytes::from("again")
    );
    assert_eq!(
        volume
            .read("a", &first, None)
            .await
            .unwrap()
            .body
            .collect()
            .await
            .unwrap(),
        Bytes::from("old")
    );
    assert_eq!(
        volume.versioning().await.unwrap(),
        BucketVersioning::Enabled
    );
}

#[test]
async fn unversioned_delete_removes_the_body_and_null_selection() {
    let volume = client(false, 2);
    let reference = create(volume.as_ref(), "a", "data").await;
    let metadata = volume.stat("a", Some("null")).await.unwrap();
    assert_eq!(metadata.version, reference);
    assert_eq!(
        volume
            .delete(WriteTarget::File("a"), &metadata)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        volume.stat("a", Some("null")).await.unwrap_err(),
        VolumeError::NotFound
    );
    assert_eq!(
        volume.read("a", &reference, None).await.unwrap_err(),
        VolumeError::NotFound
    );
    let next = create(volume.as_ref(), "a", "next").await;
    assert_eq!(next.version_id, None);
    assert_ne!(reference.etag, next.etag);
}

#[test]
async fn current_version_id_cannot_be_omitted_from_a_versioned_read_reference() {
    let volume = client(true, 2);
    let reference = create(volume.as_ref(), "a", "data").await;
    let without_id = ObjectVersion {
        etag: reference.etag.clone(),
        version_id: None,
    };
    assert_eq!(
        volume
            .read("a", &without_id, Some(4..=9))
            .await
            .unwrap_err(),
        VolumeError::InvalidResponse
    );
    assert_eq!(
        volume.stat("a", Some("null")).await.unwrap_err(),
        VolumeError::NotFound
    );
}

#[test]
async fn ranges_clip_to_eof_and_reject_stale_references_before_eof() {
    let volume = client(false, 2);
    let reference = create(volume.as_ref(), "a", "abcd").await;
    let clipped = volume
        .read("a", &reference, Some(2..=u64::MAX))
        .await
        .unwrap();
    assert_eq!(clipped.length, 2);
    assert_eq!(clipped.body.collect().await.unwrap(), Bytes::from("cd"));
    let eof = volume
        .read("a", &reference, Some(4..=u64::MAX))
        .await
        .unwrap();
    assert_eq!(eof.length, 0);
    assert_eq!(eof.version, reference);
    assert!(eof.body.collect().await.unwrap().is_empty());
    let inverted = std::ops::RangeInclusive::new(4, 3);
    assert_eq!(
        volume
            .read("a", &reference, Some(inverted))
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    volume
        .put(
            WriteTarget::File("a"),
            WriteCondition::Matches(&reference.etag),
            Bytes::from("changed"),
        )
        .await
        .unwrap();
    assert_eq!(
        volume
            .read("a", &reference, Some(100..=200))
            .await
            .unwrap_err(),
        VolumeError::Conflict
    );
    let empty = create(volume.as_ref(), "empty", "").await;
    let read = volume.read("empty", &empty, Some(0..=0)).await.unwrap();
    assert_eq!(read.length, 0);
    assert!(read.body.collect().await.unwrap().is_empty());
}

#[test]
async fn failed_conditions_leave_the_current_object_unchanged() {
    let volume = client(false, 2);
    let reference = create(volume.as_ref(), "a", "data").await;
    assert_eq!(
        volume
            .put(
                WriteTarget::File("a"),
                WriteCondition::Absent,
                Bytes::from("replace")
            )
            .await
            .unwrap_err(),
        VolumeError::Exists
    );
    assert_eq!(
        volume
            .put(
                WriteTarget::File("a"),
                WriteCondition::Matches("\"stale\""),
                Bytes::from("replace")
            )
            .await
            .unwrap_err(),
        VolumeError::Conflict
    );
    assert_eq!(
        volume
            .put(
                WriteTarget::File("missing"),
                WriteCondition::Matches(&reference.etag),
                Bytes::from("replace")
            )
            .await
            .unwrap_err(),
        VolumeError::Conflict
    );
    assert_eq!(
        volume
            .put(
                WriteTarget::File("a"),
                WriteCondition::Matches("*"),
                Bytes::from("replace")
            )
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert_eq!(volume.stat("a", None).await.unwrap().version, reference);
    assert_eq!(
        volume.stat("missing", None).await.unwrap_err(),
        VolumeError::NotFound
    );
}

#[test]
async fn delete_validates_key_etag_and_folder_marker_size_without_mutation() {
    let volume = client(false, 2);
    let reference = create(volume.as_ref(), "a/", "data").await;
    let metadata = volume.stat("a/", None).await.unwrap();
    assert_eq!(
        volume
            .delete(WriteTarget::File("b/"), &metadata)
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    let mut forged = metadata.clone();
    forged.size = 0;
    assert_eq!(
        volume
            .delete(WriteTarget::File("a/"), &forged)
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    forged = metadata.clone();
    forged.version.etag = "W/\"weak\"".into();
    assert_eq!(
        volume
            .delete(WriteTarget::File("a/"), &forged)
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    forged.version.etag = "\"stale\"".into();
    assert_eq!(
        volume
            .delete(WriteTarget::File("a/"), &forged)
            .await
            .unwrap_err(),
        VolumeError::Conflict
    );
    assert_eq!(volume.stat("a/", None).await.unwrap().version, reference);
}

#[test]
async fn list_pages_use_last_keys_even_after_the_boundary_key_is_deleted() {
    let volume = client(false, 1);
    futures::future::try_join_all(["c", "a", "b"].into_iter().map(|key| {
        volume.put(
            WriteTarget::File(key),
            WriteCondition::Absent,
            Bytes::from("data"),
        )
    }))
    .await
    .unwrap();
    let first = volume.list("", None).await.unwrap();
    assert!(
        first
            .objects
            .iter()
            .map(|object| object.key.as_ref())
            .eq(["a"])
    );
    let token = first.continuation.unwrap();
    assert_ne!(token.as_ref(), "a");
    volume
        .delete(
            WriteTarget::File("a"),
            &volume.stat("a", None).await.unwrap(),
        )
        .await
        .unwrap();
    let second = volume.list("", Some(&token)).await.unwrap();
    assert!(
        second
            .objects
            .iter()
            .map(|object| object.key.as_ref())
            .eq(["b"])
    );
    let third = volume
        .list("", second.continuation.as_deref())
        .await
        .unwrap();
    assert!(
        third
            .objects
            .iter()
            .map(|object| object.key.as_ref())
            .eq(["c"])
    );
    assert!(third.continuation.is_none());
    assert_eq!(
        volume.list("b", None).await.unwrap().objects[0]
            .key
            .as_ref(),
        "b"
    );
    assert_eq!(
        volume.list("", Some("invalid-token")).await.unwrap_err(),
        VolumeError::InvalidRequest
    );
}

#[test]
async fn probe_and_folder_rules_share_the_file_key_contract() {
    let volume = client(false, 2);
    assert_eq!(
        volume
            .put(
                WriteTarget::File(".golem/probe"),
                WriteCondition::Absent,
                Bytes::from("data")
            )
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert_eq!(
        volume
            .put(
                WriteTarget::File("folder/"),
                WriteCondition::Absent,
                Bytes::new()
            )
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert_eq!(
        volume
            .put(
                WriteTarget::File("../invalid"),
                WriteCondition::Absent,
                Bytes::from("data")
            )
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    let probe = volume
        .put(
            WriteTarget::Probe("probe"),
            WriteCondition::Absent,
            Bytes::from("probe"),
        )
        .await
        .unwrap();
    let metadata = volume.stat(".golem/probe", None).await.unwrap();
    assert_eq!(metadata.version, probe);
    volume
        .delete(WriteTarget::Probe("probe"), &metadata)
        .await
        .unwrap();
    assert_eq!(
        volume.stat(".golem/probe", None).await.unwrap_err(),
        VolumeError::NotFound
    );
    create(volume.as_ref(), "empty", "").await;
    create(volume.as_ref(), "folder/", "data").await;
    let page = volume.list("", None).await.unwrap();
    assert!(
        page.objects
            .iter()
            .map(|object| object.key.as_ref())
            .eq(["empty", "folder/"])
    );
    assert_eq!(page.objects[0].size, 0);
    assert_eq!(page.objects[1].size, 4);
}

#[test]
async fn multipart_ids_are_bound_to_keys_and_completed_parts_stay_immutable() {
    let volume = client(true, 2);
    let upload = volume
        .start_multipart(WriteTarget::File("a"))
        .await
        .unwrap()
        .into_payload();
    assert_eq!(
        volume
            .upload_part(WriteTarget::File("b"), &upload, 1, Bytes::from("data"))
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
    assert_eq!(
        volume
            .abort_multipart(WriteTarget::File("b"), &upload)
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
    let part = volume
        .upload_part(WriteTarget::File("a"), &upload, 1, Bytes::from("data"))
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("b"),
                &upload,
                std::slice::from_ref(&part),
                WriteCondition::Absent
            )
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
    let version = volume
        .complete_multipart(
            WriteTarget::File("a"),
            &upload,
            std::slice::from_ref(&part),
            WriteCondition::Absent,
        )
        .await
        .unwrap();
    assert!(version.version_id.is_some());
    assert_eq!(
        volume
            .read("a", &version, None)
            .await
            .unwrap()
            .body
            .collect()
            .await
            .unwrap(),
        Bytes::from("data")
    );
    assert_eq!(
        volume.stat("b", None).await.unwrap_err(),
        VolumeError::NotFound
    );
    assert_eq!(
        volume
            .upload_part(WriteTarget::File("a"), &upload, 1, Bytes::from("changed"))
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                std::slice::from_ref(&part),
                WriteCondition::Absent,
            )
            .await,
        Err(VolumeError::Exists)
    );
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                &[part],
                WriteCondition::Matches(&version.etag)
            )
            .await,
        Ok(version)
    );
    assert_eq!(
        volume
            .abort_multipart(WriteTarget::File("a"), &upload)
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
}

#[test]
async fn repeated_completion_returns_retained_result_without_replacing_current() {
    futures::stream::iter([(false, "a"), (true, "a"), (false, "a/"), (true, "a/")])
        .then(|(versioned, upload_key)| async move {
            let volume = client(versioned, 2);
            let target = WriteTarget::File(upload_key);
            let upload = volume.start_multipart(target).await.unwrap();
            let part = volume
                .upload_part(
                    target,
                    upload.payload(),
                    1,
                    Bytes::from_static(b"first-part-a7Z!0"),
                )
                .await
                .unwrap();
            if upload_key == "a/" {
                assert_eq!(
                    volume
                        .upload_part(
                            WriteTarget::File("a"),
                            upload.payload(),
                            1,
                            Bytes::from_static(b"wrong-key")
                        )
                        .await,
                    Err(VolumeError::NotFound)
                );
                assert_eq!(
                    volume
                        .abort_multipart(WriteTarget::File("a"), upload.payload())
                        .await,
                    Err(VolumeError::NotFound)
                );
            }
            let original = volume
                .complete_multipart(
                    target,
                    upload.payload(),
                    std::slice::from_ref(&part),
                    WriteCondition::Absent,
                )
                .await
                .unwrap();
            if upload_key == "a/" {
                assert_eq!(
                    volume
                        .complete_multipart(
                            WriteTarget::File("a"),
                            upload.payload(),
                            std::slice::from_ref(&part),
                            WriteCondition::Matches(&original.etag),
                        )
                        .await,
                    Err(VolumeError::NotFound)
                );
            }
            let mut resized = part.clone();
            resized.size = 1234567;
            assert_eq!(
                volume
                    .complete_multipart(
                        target,
                        upload.payload(),
                        &[resized],
                        WriteCondition::Matches(&original.etag),
                    )
                    .await,
                Ok(original.clone())
            );
            let mut wrong = part.clone();
            wrong.etag = "\"wrong-part\"".into();
            assert_eq!(
                volume
                    .complete_multipart(
                        target,
                        upload.payload(),
                        &[wrong],
                        WriteCondition::Matches(&original.etag),
                    )
                    .await,
                Err(VolumeError::RequestFailed)
            );
            assert_eq!(volume.stat("a", None).await.unwrap().version, original);
            assert_eq!(
                volume
                    .complete_multipart(
                        target,
                        upload.payload(),
                        std::slice::from_ref(&part),
                        WriteCondition::Matches(&original.etag),
                    )
                    .await,
                Ok(original.clone())
            );
            let held_read = volume.read("a", &original, None).await.unwrap();
            let replacement = volume
                .put(
                    WriteTarget::File("a"),
                    WriteCondition::Matches(&original.etag),
                    Bytes::from_static(b"replacement-9qX"),
                )
                .await
                .unwrap();
            assert_eq!(
                volume
                    .complete_multipart(
                        target,
                        upload.payload(),
                        std::slice::from_ref(&part),
                        WriteCondition::Matches(&original.etag),
                    )
                    .await,
                Err(VolumeError::Conflict)
            );
            assert_eq!(
                volume
                    .complete_multipart(
                        target,
                        upload.payload(),
                        &[part],
                        WriteCondition::Matches(&replacement.etag),
                    )
                    .await,
                if versioned {
                    Ok(original.clone())
                } else {
                    Err(VolumeError::NotFound)
                }
            );
            assert_eq!(volume.stat("a", None).await.unwrap().version, replacement);
            if versioned {
                assert_eq!(
                    volume
                        .stat("a", original.version_id.as_deref())
                        .await
                        .unwrap()
                        .version,
                    original
                );
                let read = volume.read("a", &original, None).await.unwrap();
                assert_eq!(read.version, original);
                assert_eq!(
                    read.body.collect().await.unwrap(),
                    Bytes::from_static(b"first-part-a7Z!0")
                );
            }
            assert_eq!(volume.stat("a/", None).await, Err(VolumeError::NotFound));
            assert_eq!(
                volume.read("a/", &original, None).await.unwrap_err(),
                VolumeError::NotFound
            );
            assert_eq!(
                volume.list("", None).await.unwrap().objects.as_ref(),
                [ListedObject {
                    key: "a".into(),
                    size: 15,
                    etag: replacement.etag.clone()
                }]
            );
            assert_eq!(
                held_read.body.collect().await.unwrap(),
                Bytes::from_static(b"first-part-a7Z!0")
            );
            assert_eq!(
                volume
                    .read("a", &replacement, None)
                    .await
                    .unwrap()
                    .body
                    .collect()
                    .await
                    .unwrap(),
                Bytes::from_static(b"replacement-9qX")
            );
        })
        .collect::<()>()
        .await;
}

#[test]
async fn abort_discards_parts_and_unknown_uploads_cannot_create_an_object() {
    let volume = client(false, 2);
    let upload = volume
        .start_multipart(WriteTarget::File("a"))
        .await
        .unwrap()
        .into_payload();
    let part = volume
        .upload_part(WriteTarget::File("a"), &upload, 1, Bytes::from("data"))
        .await
        .unwrap();
    volume
        .abort_multipart(WriteTarget::File("a"), &upload)
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                &[part],
                WriteCondition::Absent
            )
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
    assert_eq!(
        volume
            .upload_part(WriteTarget::File("a"), &upload, 1, Bytes::from("changed"))
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
    assert_eq!(
        volume
            .abort_multipart(WriteTarget::File("a"), &upload)
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
    assert_eq!(
        volume
            .upload_part(WriteTarget::File("a"), "unknown", 1, Bytes::from("data"))
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
    assert_eq!(
        volume.stat("a", None).await.unwrap_err(),
        VolumeError::NotFound
    );
}

#[test]
async fn failed_completion_preserves_the_object_and_the_open_upload() {
    let volume = client(false, 2);
    let original = create(volume.as_ref(), "a", "original").await;
    let upload = volume
        .start_multipart(WriteTarget::File("a"))
        .await
        .unwrap()
        .into_payload();
    let part = volume
        .upload_part(
            WriteTarget::File("a"),
            &upload,
            1,
            Bytes::from("replacement"),
        )
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                std::slice::from_ref(&part),
                WriteCondition::Absent
            )
            .await
            .unwrap_err(),
        VolumeError::Exists
    );
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                std::slice::from_ref(&part),
                WriteCondition::Matches("\"stale\"")
            )
            .await
            .unwrap_err(),
        VolumeError::Conflict
    );
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                std::slice::from_ref(&part),
                WriteCondition::Matches("*")
            )
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert_eq!(volume.stat("a", None).await.unwrap().version, original);
    let mut descriptor = part;
    descriptor.size += 1;
    let replacement = volume
        .complete_multipart(
            WriteTarget::File("a"),
            &upload,
            &[descriptor],
            WriteCondition::Matches(&original.etag),
        )
        .await
        .unwrap();
    assert_eq!(replacement.version_id, None);
    assert_eq!(
        volume
            .read("a", &replacement, None)
            .await
            .unwrap()
            .body
            .collect()
            .await
            .unwrap(),
        Bytes::from("replacement")
    );
}

#[test]
async fn replacement_parts_invalidate_old_part_references_without_closing_the_upload() {
    let volume = client(false, 2);
    let upload = volume
        .start_multipart(WriteTarget::File("a"))
        .await
        .unwrap()
        .into_payload();
    assert_eq!(
        volume
            .upload_part(WriteTarget::File("a"), &upload, 0, Bytes::from("bad"))
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert_eq!(
        volume
            .upload_part(WriteTarget::File("a"), &upload, 10001, Bytes::from("bad"))
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    let old = volume
        .upload_part(WriteTarget::File("a"), &upload, 1, Bytes::from("old"))
        .await
        .unwrap();
    let new = volume
        .upload_part(WriteTarget::File("a"), &upload, 1, Bytes::from("new"))
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                &[old],
                WriteCondition::Absent
            )
            .await
            .unwrap_err(),
        VolumeError::RequestFailed
    );
    assert_eq!(
        volume
            .complete_multipart(WriteTarget::File("a"), &upload, &[], WriteCondition::Absent)
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    let version = volume
        .complete_multipart(
            WriteTarget::File("a"),
            &upload,
            &[new],
            WriteCondition::Absent,
        )
        .await
        .unwrap();
    assert_eq!(
        volume
            .read("a", &version, None)
            .await
            .unwrap()
            .body
            .collect()
            .await
            .unwrap(),
        Bytes::from("new")
    );
}

#[test]
async fn empty_slash_multipart_cannot_publish_a_folder_marker() {
    futures::stream::iter([false, true])
        .then(|versioned| async move {
            let volume = client(versioned, 2);
            let target = WriteTarget::File("a/");
            let upload = volume.start_multipart(target).await.unwrap();
            let empty = volume
                .upload_part(target, upload.payload(), 1, Bytes::new())
                .await
                .unwrap();
            assert_eq!(
                volume
                    .complete_multipart(target, upload.payload(), &[empty], WriteCondition::Absent)
                    .await,
                Err(VolumeError::InvalidRequest)
            );
            assert_eq!(volume.stat("a/", None).await, Err(VolumeError::NotFound));
            let part = volume
                .upload_part(
                    target,
                    upload.payload(),
                    1,
                    Bytes::from_static(b"slash-part-a7Z!"),
                )
                .await
                .unwrap();
            assert_eq!(part.size, 15);
            let version = volume
                .complete_multipart(target, upload.payload(), &[part], WriteCondition::Absent)
                .await
                .unwrap();
            let metadata = volume.stat("a", None).await.unwrap();
            assert_eq!(metadata.key.as_ref(), "a");
            assert_eq!(metadata.size, 15);
            assert_eq!(metadata.version, version);
            assert_eq!(version.version_id.is_some(), versioned);
            let read = volume.read("a", &version, None).await.unwrap();
            assert_eq!(read.version, version);
            assert_eq!(read.length, 15);
            assert_eq!(
                read.body.collect().await.unwrap(),
                Bytes::from_static(b"slash-part-a7Z!")
            );
            let eof = volume.read("a", &version, Some(15..=19)).await.unwrap();
            assert_eq!(eof.version, version);
            assert_eq!(eof.length, 0);
            assert!(eof.body.collect().await.unwrap().is_empty());
            assert_eq!(volume.stat("a/", None).await, Err(VolumeError::NotFound));
            assert_eq!(
                volume.read("a/", &version, None).await.unwrap_err(),
                VolumeError::NotFound
            );
            let expected = [ListedObject {
                key: "a".into(),
                size: 15,
                etag: version.etag.clone(),
            }];
            assert_eq!(
                volume.list("", None).await.unwrap().objects.as_ref(),
                expected
            );

            let rejected = volume.start_multipart(target).await.unwrap();
            let part = volume
                .upload_part(
                    target,
                    rejected.payload(),
                    1,
                    Bytes::from_static(b"must-not-replace"),
                )
                .await
                .unwrap();
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
            assert_eq!(volume.stat("a", None).await.unwrap(), metadata);
            let read = volume.read("a", &version, None).await.unwrap();
            assert_eq!(read.version, version);
            assert_eq!(
                read.body.collect().await.unwrap(),
                Bytes::from_static(b"slash-part-a7Z!")
            );
            assert_eq!(
                volume.list("", None).await.unwrap().objects.as_ref(),
                expected
            );
        })
        .collect::<()>()
        .await;
}

#[test]
async fn a_started_read_keeps_its_bytes_after_an_unversioned_replacement() {
    let volume = client(false, 2);
    let original = create(volume.as_ref(), "a", "original").await;
    let read = volume.read("a", &original, None).await.unwrap();
    volume
        .put(
            WriteTarget::File("a"),
            WriteCondition::Matches(&original.etag),
            Bytes::from("replacement"),
        )
        .await
        .unwrap();
    assert_eq!(read.length, 8);
    assert_eq!(read.version, original);
    assert_eq!(read.body.collect().await.unwrap(), Bytes::from("original"));
}

#[test]
async fn multipart_requires_ordered_existing_parts_then_assembles_their_bytes() {
    let volume = client(false, 2);
    let upload = volume
        .start_multipart(WriteTarget::File("a"))
        .await
        .unwrap()
        .into_payload();
    let first_size = 5 * 1024 * 1024;
    let second = volume
        .upload_part(WriteTarget::File("a"), &upload, 2, Bytes::from("Z"))
        .await
        .unwrap();
    let small = volume
        .upload_part(WriteTarget::File("a"), &upload, 1, Bytes::from("short"))
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                &[small, second.clone()],
                WriteCondition::Absent,
            )
            .await,
        Err(VolumeError::RequestFailed)
    );
    let first = volume
        .upload_part(
            WriteTarget::File("a"),
            &upload,
            1,
            Bytes::from(vec![b'a'; first_size]),
        )
        .await
        .unwrap();
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                &[second.clone(), first.clone()],
                WriteCondition::Absent
            )
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                &[first.clone(), first.clone()],
                WriteCondition::Absent
            )
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    let mut missing = second.clone();
    missing.number = 3;
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("a"),
                &upload,
                &[first.clone(), missing],
                WriteCondition::Absent
            )
            .await
            .unwrap_err(),
        VolumeError::RequestFailed
    );
    assert_eq!(
        volume.stat("a", None).await.unwrap_err(),
        VolumeError::NotFound
    );
    let reference = volume
        .complete_multipart(
            WriteTarget::File("a"),
            &upload,
            &[first, second],
            WriteCondition::Absent,
        )
        .await
        .unwrap();
    assert_eq!(
        volume.stat("a", None).await.unwrap().size,
        first_size as u64 + 1
    );
    let read = volume
        .read("a", &reference, Some(first_size as u64 - 2..=u64::MAX))
        .await
        .unwrap();
    assert_eq!(read.length, 3);
    assert_eq!(read.body.collect().await.unwrap(), Bytes::from("aaZ"));
}

#[test]
async fn invalid_read_references_fail_even_when_the_key_is_missing() {
    let volume = client(false, 2);
    let weak = ObjectVersion {
        etag: "W/\"weak\"".into(),
        version_id: None,
    };
    assert_eq!(
        volume
            .read("missing", &weak, Some(0..=0))
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    let valid = ObjectVersion {
        etag: "\"valid\"".into(),
        version_id: None,
    };
    assert_eq!(
        volume.read("../invalid", &valid, None).await.unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert_eq!(
        volume.stat("../invalid", None).await.unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert_eq!(
        volume.list("../invalid", None).await.unwrap_err(),
        VolumeError::InvalidRequest
    );
}

#[test]
async fn multipart_descriptor_size_does_not_change_actual_empty_publication() {
    futures::stream::iter([false, true])
        .then(|versioned| async move {
            let volume = client(versioned, 2);
            let target = WriteTarget::File("marker/");
            let upload = volume.start_multipart(target).await.unwrap();
            let mut part = volume
                .upload_part(target, upload.payload(), 1, Bytes::new())
                .await
                .unwrap();
            part.size = 1;
            let reference = volume
                .complete_multipart(target, upload.payload(), &[part], WriteCondition::Absent)
                .await
                .unwrap();
            let metadata = volume.stat("marker", None).await.unwrap();
            assert_eq!(metadata.key.as_ref(), "marker");
            assert_eq!(metadata.size, 0);
            assert_eq!(metadata.version, reference);
            assert_eq!(reference.version_id.is_some(), versioned);
            assert_eq!(
                volume.stat("marker/", None).await,
                Err(VolumeError::NotFound)
            );
            futures::stream::iter([None, Some(0..=0)])
                .then(|range| {
                    let volume = &volume;
                    let reference = &reference;
                    async move {
                        let read = volume
                            .read("marker", reference, range.clone())
                            .await
                            .unwrap();
                        assert_eq!(&read.version, reference);
                        assert_eq!(read.length, 0);
                        assert!(read.body.collect().await.unwrap().is_empty());
                        assert_eq!(
                            volume.read("marker/", reference, range).await.unwrap_err(),
                            VolumeError::NotFound
                        );
                    }
                })
                .collect::<()>()
                .await;
            assert_eq!(
                volume.list("", None).await.unwrap().objects.as_ref(),
                [ListedObject {
                    key: "marker".into(),
                    size: 0,
                    etag: reference.etag
                }]
            );
        })
        .collect::<()>()
        .await;
}
