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

use pretty_assertions::assert_eq;
use test_r::test;

#[test]
fn a_withdrawal_stops_a_call_and_its_deadline_fails_a_call_whose_run_failed() {
    assert_eq!(
        [
            super::withdrawn_call(super::Withdrawal::Stopped, None),
            super::withdrawn_call(super::Withdrawal::Stopped, Some("last")),
            super::withdrawn_call(super::Withdrawal::Deadline, None),
            super::withdrawn_call(super::Withdrawal::Deadline, Some("last")),
        ],
        [
            super::WithdrawnCall::Stopped(super::Withdrawal::Stopped),
            super::WithdrawnCall::Stopped(super::Withdrawal::Stopped),
            super::WithdrawnCall::Stopped(super::Withdrawal::Deadline),
            super::WithdrawnCall::Failed("last"),
        ]
    );
}
