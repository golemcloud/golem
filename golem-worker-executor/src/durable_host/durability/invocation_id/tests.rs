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
fn invocation_ids_number_the_calls_under_their_anchor() {
    let invocation = OplogIndex::from_u64(5);
    let mut ordinals = HashMap::new();

    let first = next_invocation_id(&mut ordinals, invocation).unwrap();
    let second = next_invocation_id(&mut ordinals, invocation).unwrap();

    assert_eq!(first, DurableInvocationId::new(invocation, 0));
    assert_eq!(second, DurableInvocationId::new(invocation, 1));

    let mut replay_ordinals = HashMap::new();
    assert_eq!(
        next_invocation_id(&mut replay_ordinals, invocation).unwrap(),
        first
    );
    assert_eq!(
        next_invocation_id(&mut replay_ordinals, invocation).unwrap(),
        second
    );
}

#[test]
fn nested_invocation_ordinals_do_not_shift_root_siblings() {
    let invocation = OplogIndex::from_u64(5);
    let mut live_ordinals = HashMap::new();
    let outer = next_invocation_id(&mut live_ordinals, invocation).unwrap();
    let outer_start = OplogIndex::from_u64(6);
    let nested = next_invocation_id(&mut live_ordinals, outer_start).unwrap();
    let root_sibling = next_invocation_id(&mut live_ordinals, invocation).unwrap();

    let mut replay_ordinals = HashMap::new();
    let replayed_outer = next_invocation_id(&mut replay_ordinals, invocation).unwrap();
    // A completed outer invocation is replayed without executing its body, so the nested
    // calls take no ordinal.
    let replayed_root_sibling = next_invocation_id(&mut replay_ordinals, invocation).unwrap();

    assert_eq!(replayed_outer, outer);
    assert_eq!(replayed_root_sibling, root_sibling);
    assert_eq!(nested, DurableInvocationId::new(outer_start, 0));
}
