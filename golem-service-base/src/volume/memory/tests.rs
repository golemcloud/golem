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
use futures::TryStreamExt;
use test_r::test;

async fn bytes(stream: ReadStream) -> Bytes {
    stream
        .try_fold(bytes::BytesMut::new(), |mut bytes, chunk| async move {
            bytes.extend_from_slice(&chunk);
            Ok(bytes)
        })
        .await
        .unwrap()
        .freeze()
}

#[test]
async fn an_open_file_and_recovery_read_the_selected_state_until_it_is_forgotten() {
    let (volume, control) = InMemoryVolume::new(true, 2).unwrap();
    let path = VolumePath::new("reports/a").unwrap();
    let state = control
        .write(&path, Bytes::from_static(b"first-73"))
        .unwrap();
    let opened = volume.open(&path).await.unwrap();
    assert_eq!(opened.state(), &state);
    assert_eq!(opened.size(), 8);
    assert!(opened.replayable());
    control
        .write(&path, Bytes::from_static(b"replacement-91"))
        .unwrap();
    assert_eq!(
        bytes(opened.read(2..u64::MAX).await.unwrap()).await,
        "rst-73"
    );
    let restored = volume.open_recorded(&path, &state).await.unwrap();
    assert_eq!(bytes(restored.read(0..8).await.unwrap()).await, "first-73");
    assert_eq!(
        volume.stat(&path).await.unwrap(),
        Some(Node::File { size: 14 })
    );
    control.forget(&path, &state);
    assert_eq!(
        volume.open_recorded(&path, &state).await.unwrap_err(),
        VolumeError::Changed
    );
    assert_eq!(opened.read(0..1).await.err(), Some(VolumeError::Changed));
}

#[test]
async fn directories_have_direct_children_and_cursors_do_not_cross_directories() {
    let (volume, control) = InMemoryVolume::new(false, 2).unwrap();
    [
        "corpus/z",
        "corpus/a/first",
        "corpus/a/second",
        "corpus/m",
        "corpus2/neighbor",
    ]
    .into_iter()
    .for_each(|name| {
        control
            .write(
                &VolumePath::new(name).unwrap(),
                Bytes::from_static(b"abcde"),
            )
            .unwrap();
    });
    let root = VolumePath::new("").unwrap();
    let corpus = VolumePath::new("corpus").unwrap();
    assert_eq!(volume.stat(&root).await.unwrap(), Some(Node::Directory));
    assert_eq!(volume.stat(&corpus).await.unwrap(), Some(Node::Directory));
    assert_eq!(
        volume
            .stat(&VolumePath::new("corpu").unwrap())
            .await
            .unwrap(),
        None
    );
    let first = volume.list(&corpus, None).await.unwrap();
    assert!(
        first
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry.node.clone()))
            .eq([
                ("corpus/a", Node::Directory),
                ("corpus/m", Node::File { size: 5 })
            ])
    );
    let after = first.after.unwrap();
    let second = volume.list(&corpus, Some(&after)).await.unwrap();
    assert!(
        second
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .eq(["corpus/z"])
    );
    assert!(second.after.is_none());
    assert_eq!(
        volume.list(&root, Some(&after)).await,
        Err(VolumeError::InvalidRequest)
    );
    assert_eq!(
        volume.list(&corpus, Some(&Cursor::new("invalid"))).await,
        Err(VolumeError::InvalidRequest)
    );
    assert_eq!(
        volume.open(&corpus).await.unwrap_err(),
        VolumeError::NotFound
    );
}

#[test]
async fn unretained_changes_fail_reads_but_empty_ranges_consume_no_failure() {
    let (volume, control) = InMemoryVolume::new(false, 3).unwrap();
    let path = VolumePath::new("report").unwrap();
    let state = control.write(&path, Bytes::from_static(b"old-73")).unwrap();
    let file = volume.open(&path).await.unwrap();
    assert!(!file.replayable());
    control.fail_next(VolumeError::Unanswered);
    assert!(
        bytes(file.read(6..u64::MAX).await.unwrap())
            .await
            .is_empty()
    );
    assert!(bytes(file.read(2..2).await.unwrap()).await.is_empty());
    assert_eq!(
        file.read(std::ops::Range { start: 3, end: 2 }).await.err(),
        Some(VolumeError::InvalidRequest)
    );
    assert_eq!(file.read(0..1).await.err(), Some(VolumeError::Unanswered));
    let stream = file.read(0..6).await.unwrap();
    control
        .write(&path, Bytes::from_static(b"changed-91"))
        .unwrap();
    assert_eq!(bytes(stream).await, "old-73");
    assert_eq!(file.read(0..1).await.err(), Some(VolumeError::Changed));
    assert_eq!(
        volume.open_recorded(&path, &state).await.unwrap_err(),
        VolumeError::Changed
    );
    assert_eq!(
        volume
            .open(&VolumePath::new("missing").unwrap())
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
    control.fail_next(VolumeError::Denied {
        action: "read".into(),
    });
    assert_eq!(
        volume.stat(&path).await,
        Err(VolumeError::Denied {
            action: "read".into()
        })
    );
    assert_eq!(
        volume.stat(&path).await.unwrap(),
        Some(Node::File { size: 10 })
    );
}
