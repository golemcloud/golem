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
use golem::workerexecutor::v1::get_oplog_entry_response::Result as Frame;
use test_r::test;

async fn frame_chunks(encoded: Vec<u8>, chunk_size: usize) -> Vec<Vec<u8>> {
    oplog_entry_frames(encoded, chunk_size, ())
        .map(|frame| match frame.expect("frame").result {
            Some(Frame::Chunk(chunk)) => chunk,
            other => panic!("expected a chunk frame, got {other:?}"),
        })
        .collect()
        .await
}

#[test]
async fn oplog_entry_frames_split_the_encoded_entry_into_chunks_of_the_chunk_size() {
    let encoded: Vec<u8> = (0..=10u8).collect();

    let chunks = frame_chunks(encoded.clone(), 4).await;
    assert_eq!(
        chunks,
        vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7], vec![8, 9, 10]]
    );

    let chunks = frame_chunks(encoded[..8].to_vec(), 4).await;
    assert_eq!(chunks, vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7]]);

    assert!(frame_chunks(Vec::new(), 4).await.is_empty());
}

#[test]
async fn oplog_entry_frames_hold_the_admission_until_the_stream_is_dropped() {
    let reads = Arc::new(Semaphore::new(1));
    let admission = reads.clone().acquire_owned().await.unwrap();
    let mut frames = Box::pin(oplog_entry_frames(vec![0; 12], 4, admission));

    assert!(frames.next().await.is_some());
    assert_eq!(reads.available_permits(), 0);

    drop(frames);
    assert_eq!(reads.available_permits(), 1);
}
