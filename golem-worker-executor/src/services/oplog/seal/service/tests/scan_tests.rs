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
use golem_common::model::ScanCursor;
use test_r::test;

async fn finish_scan(
    f: &Fixture,
    oplog: &dyn OplogService,
    mut cursor: ScanCursor,
) -> Vec<OwnedAgentId> {
    let mut ids = Vec::new();
    for _ in 0..30 {
        let page = oplog
            .scan_for_type(
                &f.id().environment_id(),
                f.id().agent_id.type_id().unwrap(),
                Some(f.metadata.agent_mode),
                cursor.clone(),
                1,
            )
            .await
            .unwrap();
        assert!(page.examined_keys <= 1);
        ids.extend(page.agent_ids);
        if page.cursor.is_finished() {
            return ids;
        }
        assert_ne!(page.cursor, cursor);
        cursor = page.cursor;
    }
    panic!("scan did not finish");
}

#[test]
async fn sealed_scan_marker_keeps_listing_stable_across_release_and_unseal() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            let f = Fixture::with_mode(indexed.clone(), mode).await;
            let oplog = f.oplog_service().await;
            let seal = f.service();
            assert_eq!(
                finish_scan(&f, &oplog, ScanCursor::default()).await,
                vec![f.id()]
            );
            let before_seal = oplog
                .scan_for_type(
                    &f.id().environment_id(),
                    f.id().agent_id.type_id().unwrap(),
                    Some(mode),
                    ScanCursor::default(),
                    1,
                )
                .await
                .unwrap();
            assert!(before_seal.agent_ids.is_empty());
            let pending = seal
                .prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                finish_scan(&f, &oplog, before_seal.cursor).await,
                vec![f.id()]
            );
            // The marker now suppresses the indexed copy before release.
            assert_eq!(
                finish_scan(&f, &oplog, ScanCursor::default()).await,
                vec![f.id()]
            );
            let first = oplog
                .scan_for_type(
                    &f.id().environment_id(),
                    f.id().agent_id.type_id().unwrap(),
                    Some(mode),
                    ScanCursor::default(),
                    1,
                )
                .await
                .unwrap();
            assert!(first.agent_ids.is_empty());
            seal.remove_moved_copies(&pending).await.unwrap();
            assert!(seal.release(&pending).await.unwrap());
            assert_eq!(finish_scan(&f, &oplog, first.cursor).await, vec![f.id()]);
            if mode == AgentMode::Durable {
                let first = oplog
                    .scan_for_type(
                        &f.id().environment_id(),
                        f.id().agent_id.type_id().unwrap(),
                        Some(mode),
                        ScanCursor::default(),
                        1,
                    )
                    .await
                    .unwrap();
                assert!(first.agent_ids.is_empty());
                seal.unseal(&f.id(), ShardEpoch(6)).await.unwrap();
                assert_eq!(finish_scan(&f, &oplog, first.cursor).await, vec![f.id()]);
                assert_eq!(
                    finish_scan(&f, &oplog, ScanCursor::default()).await,
                    vec![f.id()]
                );
            }
        }
    }
}
