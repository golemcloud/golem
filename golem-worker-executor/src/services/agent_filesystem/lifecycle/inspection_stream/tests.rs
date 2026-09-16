use super::super::tests::{native_resident, resident, sandbox_attributes, unsupported_allocation};
use super::super::{delete, resident_generation_handle, seal};
use super::*;
use crate::sandbox_filesystem::{SandboxObjectKind, SandboxOpened};
use futures::StreamExt;
use golem_common::model::filesystem::{
    FileByteSelection, FileReadError, FileReadExtent, FileReadHead,
};
use golem_service_base::model::FileReadResponse;
use test_r::{test, timeout};

#[test]
#[timeout("30s")]
async fn abandoned_read_does_not_begin_filesystem_io() {
    let (resident, control, _) = resident(Err(unsupported_allocation())).await;
    let before = control.calls();
    let handle = resident_generation_handle(&resident);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    drop(receiver);
    produce_file_read(&handle, "/file", FileByteSelection::Full, sender).await;
    assert_eq!(control.calls(), before);
    control.push_delete_and_verify(Ok(()));
    delete(seal(resident)).await.unwrap();
}

async fn start_read<Adapter: SandboxFilesystemAdapter>(
    handle: FilesystemGenerationHandle<Adapter>,
    selection: FileByteSelection,
) -> (tokio::task::JoinHandle<()>, FileReadResponse) {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        produce_file_read(&handle, "/file", selection, sender).await;
    });
    (task, receiver.await.unwrap().unwrap())
}

#[test]
#[timeout("30s")]
async fn stream_selects_exact_bytes_in_bounded_chunks_and_finishes_on_eof() {
    let parent = tempfile::tempdir().unwrap();
    let (resident, root) = native_resident(parent.path()).await;
    let content: Vec<u8> = (0..131089).map(|i| (i % 251) as u8).collect();
    std::fs::write(root.join("file"), &content).unwrap();
    for (selection, offset, end) in [
        (FileByteSelection::Full, 0, 131089),
        (
            FileByteSelection::Bounded {
                start: 3,
                end_inclusive: 65541,
            },
            3,
            65542,
        ),
        (FileByteSelection::OpenEnded { start: 65531 }, 65531, 131089),
        (FileByteSelection::Suffix { length: 65539 }, 65550, 131089),
    ] {
        let (producer, mut response) =
            start_read(resident_generation_handle(&resident), selection).await;
        let FileReadHead::File(metadata) = response.head else {
            panic!("regular file head")
        };
        assert_eq!(metadata.total_size, 131089);
        assert_eq!(
            metadata.selection,
            FileReadExtent::Selected {
                offset: offset as u64,
                length: (end - offset) as u64
            }
        );
        let mut actual = Vec::new();
        while let Some(chunk) = response.body.next().await {
            let chunk = chunk.unwrap();
            assert!(!chunk.is_empty() && chunk.len() <= 65536);
            actual.extend_from_slice(&chunk);
        }
        assert_eq!(actual, content[offset..end]);
        // Keep the exhausted body allocated: EOF, rather than Drop, must release the producer.
        producer.await.unwrap();
        assert!(response.body.next().await.is_none());
    }
    delete(seal(resident)).await.unwrap();
}

#[test]
#[timeout("30s")]
async fn last_chunk_does_not_release_turn_until_eof_is_polled() {
    let parent = tempfile::tempdir().unwrap();
    let (resident, root) = native_resident(parent.path()).await;
    std::fs::write(root.join("file"), b"abcdef").unwrap();
    let (producer, mut response) = start_read(
        resident_generation_handle(&resident),
        FileByteSelection::Full,
    )
    .await;
    assert_eq!(
        response.body.next().await.unwrap().unwrap().as_ref(),
        b"abcdef"
    );
    tokio::task::yield_now().await;
    assert!(
        !producer.is_finished(),
        "last chunk must not release the read turn"
    );
    assert!(response.body.next().await.is_none());
    producer.await.unwrap();
    assert!(response.body.next().await.is_none());
    delete(seal(resident)).await.unwrap();
}

#[test]
fn backpressured_read_waits_for_consumer_without_expiring() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::pause();
            let parent = tempfile::tempdir().unwrap();
            let (resident, root) = native_resident(parent.path()).await;
            let content: Vec<u8> = (0..3 * 65536).map(|i| (i % 251) as u8).collect();
            std::fs::write(root.join("file"), &content).unwrap();
            let (producer, mut response) = start_read(
                resident_generation_handle(&resident),
                FileByteSelection::Full,
            )
            .await;
            let mut actual = response.body.next().await.unwrap().unwrap().to_vec();
            tokio::time::advance(std::time::Duration::from_secs(61)).await;
            assert!(
                !producer.is_finished(),
                "backpressure must retain the read turn"
            );
            while let Some(chunk) = response.body.next().await {
                actual.extend_from_slice(&chunk.unwrap());
            }
            assert_eq!(actual, content);
            producer.await.unwrap();
            delete(seal(resident)).await.unwrap();
        });
}

#[test]
#[timeout("30s")]
async fn head_only_empty_and_unsatisfiable_reads_release_without_body_poll() {
    let parent = tempfile::tempdir().unwrap();
    let (resident, root) = native_resident(parent.path()).await;
    for (bytes, selection, expected) in [
        (
            &b"abcdef"[..],
            FileByteSelection::MetadataOnly,
            FileReadExtent::Selected {
                offset: 0,
                length: 0,
            },
        ),
        (
            &b""[..],
            FileByteSelection::Full,
            FileReadExtent::Selected {
                offset: 0,
                length: 0,
            },
        ),
        (
            &b"abcdef"[..],
            FileByteSelection::OpenEnded { start: 6 },
            FileReadExtent::Unsatisfiable,
        ),
    ] {
        std::fs::write(root.join("file"), bytes).unwrap();
        let (producer, mut response) =
            start_read(resident_generation_handle(&resident), selection).await;
        producer.await.unwrap();
        let FileReadHead::File(metadata) = response.head else {
            panic!("regular file head")
        };
        assert_eq!(metadata.total_size, bytes.len() as u64);
        assert_eq!(metadata.selection, expected);
        assert!(response.body.next().await.is_none());
    }
    delete(seal(resident)).await.unwrap();
}

#[test]
#[timeout("30s")]
async fn dropping_body_releases_backpressured_producer_and_aborting_producer_discards_bytes() {
    let parent = tempfile::tempdir().unwrap();
    let (resident, root) = native_resident(parent.path()).await;
    std::fs::write(root.join("file"), vec![42; 3 * 65536]).unwrap();
    let (producer, response) = start_read(
        resident_generation_handle(&resident),
        FileByteSelection::Full,
    )
    .await;
    drop(response);
    producer.await.unwrap();

    let (producer, mut response) = start_read(
        resident_generation_handle(&resident),
        FileByteSelection::Full,
    )
    .await;
    producer.abort();
    assert!(producer.await.unwrap_err().is_cancelled());
    assert_eq!(
        response.body.next().await,
        Some(Err(FileReadError::Lifecycle))
    );
    assert!(response.body.next().await.is_none());
    delete(seal(resident)).await.unwrap();
}

#[test]
#[timeout("30s")]
async fn u64_suffix_cursor_handles_short_reads_without_allocating_total_size() {
    let (resident, control, _) = resident(Err(unsupported_allocation())).await;
    control.push_open(Ok(SandboxOpened::scripted_file(12)));
    control.push_close(Ok(()));
    let mut attrs = sandbox_attributes(SandboxObjectKind::File);
    attrs.size = u64::MAX;
    control.push_get_attributes(Ok(attrs));
    control.push_read(Ok(Bytes::from_static(b"ab")));
    control.push_read(Ok(Bytes::from_static(b"cde")));
    let (producer, mut response) = start_read(
        resident_generation_handle(&resident),
        FileByteSelection::Suffix { length: 5 },
    )
    .await;
    assert_eq!(response.body.next().await.unwrap().unwrap().as_ref(), b"ab");
    assert_eq!(
        response.body.next().await.unwrap().unwrap().as_ref(),
        b"cde"
    );
    assert!(response.body.next().await.is_none());
    producer.await.unwrap();
    let calls = control.calls();
    let reads: Vec<_> = calls
        .iter()
        .filter(|call| call.starts_with("read("))
        .collect();
    assert_eq!(reads.len(), 2);
    assert!(reads[0].contains("offset: 18446744073709551610, length: 5"));
    assert!(reads[1].contains("offset: 18446744073709551612, length: 3"));
    control.push_delete_and_verify(Ok(()));
    delete(seal(resident)).await.unwrap();
}

#[test]
#[timeout("30s")]
async fn premature_storage_eof_is_an_error_not_a_short_success() {
    let (resident, control, _) = resident(Err(unsupported_allocation())).await;
    control.push_open(Ok(SandboxOpened::scripted_file(12)));
    control.push_close(Ok(()));
    let mut attrs = sandbox_attributes(SandboxObjectKind::File);
    attrs.size = 5;
    control.push_get_attributes(Ok(attrs));
    control.push_read(Ok(Bytes::new()));
    let (producer, mut response) = start_read(
        resident_generation_handle(&resident),
        FileByteSelection::Full,
    )
    .await;
    assert_eq!(
        response.body.next().await,
        Some(Err(FileReadError::Storage))
    );
    assert!(response.body.next().await.is_none());
    producer.await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(resident)).await.unwrap();
}
