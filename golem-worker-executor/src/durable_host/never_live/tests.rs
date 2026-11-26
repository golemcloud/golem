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

use super::admits;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::host_functions::HostFunctionName;
use test_r::test;

const ALL_TYPES: [DurableFunctionType; 4] = [
    DurableFunctionType::ReadLocal,
    DurableFunctionType::WriteLocal,
    DurableFunctionType::ReadRemote,
    DurableFunctionType::WriteRemote,
];

fn every_host_function() -> Vec<HostFunctionName> {
    HostFunctionName::NAMED
        .iter()
        .cloned()
        .chain([HostFunctionName::Custom("wall_clock::now".to_string())])
        .collect()
}

/// A never-live owner admits a call that executes now only when it is `ReadLocal` and not
/// `Custom`: a write, a remote read and a guest-named function are refused, also a `Custom`
/// function that the guest names after a local one.
#[test]
fn a_call_that_executes_now_is_admitted_only_when_it_is_a_local_read() {
    for name in every_host_function() {
        for function_type in &ALL_TYPES {
            let expected = matches!(function_type, DurableFunctionType::ReadLocal)
                && !matches!(name, HostFunctionName::Custom(_));
            assert_eq!(
                admits(true, true, function_type, &name),
                expected,
                "{name} {function_type:?}"
            );
        }
    }
    assert!(!admits(
        true,
        true,
        &DurableFunctionType::ReadRemote,
        &HostFunctionName::KeyvalueEventualGet
    ));
    assert!(admits(
        true,
        true,
        &DurableFunctionType::ReadLocal,
        &HostFunctionName::WallClockNow
    ));
}

/// GOL-2143 P2: a never-live owner admits a call that only returns its recorded result, of every
/// function and type, `Custom` included.
#[test]
fn a_recorded_result_is_admitted_for_every_call() {
    for name in every_host_function() {
        for function_type in &ALL_TYPES {
            assert!(admits(true, false, function_type, &name), "{name}");
        }
    }
}

/// GOL-2143 P9: an unmarked owner admits every call, recorded or executing, so its snapshot hooks
/// run as before.
#[test]
fn an_unmarked_owner_admits_every_call() {
    for name in every_host_function() {
        for function_type in &ALL_TYPES {
            for executes_now in [false, true] {
                assert!(admits(false, executes_now, function_type, &name), "{name}");
            }
        }
    }
}
