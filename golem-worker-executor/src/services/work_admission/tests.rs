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
use test_r::test;

#[test]
async fn closing_refuses_new_work_without_waiting_for_an_existing_lease() {
    let admission = WorkAdmission::default();
    let lease = admission.enter().unwrap();
    let retained = lease.clone();
    admission.close();
    assert!(admission.enter().is_none());
    let waiting = admission.wait();
    tokio::pin!(waiting);
    assert!(futures::poll!(&mut waiting).is_pending());
    drop(lease);
    assert!(futures::poll!(&mut waiting).is_pending());
    drop(retained);
    waiting.await;
    admission.close();
    admission.wait().await;
}
