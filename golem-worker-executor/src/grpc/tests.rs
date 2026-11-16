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

mod caller_decoding {
    use super::*;
    use crate::test_callers::sample_caller;
    use golem_api_grpc::proto::golem::component as proto;
    use golem_common::model::caller::Caller;
    use test_r::test;

    #[test]
    fn extract_caller_refuses_a_missing_caller() {
        let error = extract_caller(&None).unwrap_err();
        assert!(error.to_string().contains("caller is missing"), "{error}");
    }

    #[test]
    fn extract_caller_refuses_a_caller_that_does_not_decode() {
        let valid: proto::Caller = sample_caller().into();

        let mut bad_claims = valid.clone();
        let Some(proto::principal::Value::User(user)) = bad_claims
            .original
            .as_mut()
            .and_then(|principal| principal.value.as_mut())
        else {
            panic!("the sample caller starts with a user");
        };
        user.claims = "[1]".to_string();
        let error = extract_caller(&Some(bad_claims)).unwrap_err();
        assert!(
            error.to_string().contains("caller does not decode"),
            "{error}"
        );

        let mut empty_principal = valid;
        empty_principal.immediate = Some(proto::Principal { value: None });
        let error = extract_caller(&Some(empty_principal)).unwrap_err();
        assert!(
            error.to_string().contains("caller does not decode"),
            "{error}"
        );
    }

    #[test]
    fn extract_caller_decodes_a_valid_caller() {
        let caller = sample_caller();
        let decoded = extract_caller(&Some(caller.clone().into())).unwrap();
        assert_eq!(decoded, caller);
    }

    #[test]
    fn can_start_worker_refuses_a_missing_caller() {
        use crate::grpc::invocation::CanStartWorker;
        use golem_api_grpc::proto::golem::worker::InvocationStart;
        use golem_api_grpc::proto::golem::workerexecutor::v1::{
            DeliverCardTransferRequest, GetFileContentsRequest,
        };

        let start = InvocationStart::default();
        assert!(start.caller().is_err());
        let file_contents = GetFileContentsRequest::default();
        assert!(file_contents.caller().is_err());

        let with_caller = InvocationStart {
            caller: Some(sample_caller().into()),
            ..Default::default()
        };
        assert_eq!(with_caller.caller().unwrap(), sample_caller());

        // A card transfer has no caller field and is explicitly anonymous.
        assert_eq!(
            DeliverCardTransferRequest::default().caller().unwrap(),
            Caller::anonymous()
        );
    }
}
