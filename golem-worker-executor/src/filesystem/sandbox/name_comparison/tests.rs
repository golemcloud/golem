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

use super::SandboxNameComparisonKey;
use std::hash::Hash;
use std::hash::Hasher;
use test_r::test;

fn name_comparison_hash(key: &SandboxNameComparisonKey) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn exact_native_name_mode_distinguishes_siblings() {
    let first = SandboxNameComparisonKey::scripted_exact(1, "first");
    let same = SandboxNameComparisonKey::scripted_exact(1, "first");
    let sibling = SandboxNameComparisonKey::scripted_exact(1, "second");

    assert!(first == same);
    assert_eq!(name_comparison_hash(&first), name_comparison_hash(&same));
    assert!(first != sibling);
    assert!(!first.may_conflict_with(&sibling));
}

#[test]
fn windows_insensitive_native_name_mode_compares_equivalent_names() {
    let first = SandboxNameComparisonKey::scripted_windows_insensitive(1, "CaseName");
    let equivalent = SandboxNameComparisonKey::scripted_windows_insensitive(1, "casename");
    let distinct = SandboxNameComparisonKey::scripted_windows_insensitive(1, "other");

    assert!(first == equivalent);
    assert_eq!(
        name_comparison_hash(&first),
        name_comparison_hash(&equivalent)
    );
    assert!(first != distinct);
    assert!(!first.may_conflict_with(&distinct));
}

#[test]
fn conservative_native_name_mode_is_scoped_to_one_parent() {
    let first = SandboxNameComparisonKey::scripted_conservative(1, "first");
    let sibling = SandboxNameComparisonKey::scripted_conservative(1, "second");
    let other_parent = SandboxNameComparisonKey::scripted_conservative(2, "first");
    let exact = SandboxNameComparisonKey::scripted_exact(1, "third");

    assert!(first == sibling);
    assert_eq!(name_comparison_hash(&first), name_comparison_hash(&sibling));
    assert!(first != other_parent);
    assert!(!first.may_conflict_with(&other_parent));
    assert!(first != exact);
    assert!(first.may_conflict_with(&exact));
}

#[test]
fn apfs_conservative_keys_cover_normalization_aliases_and_parent_scope() {
    let composed = SandboxNameComparisonKey::scripted_conservative(1, "caf\u{00e9}.txt");
    let decomposed = SandboxNameComparisonKey::scripted_conservative(1, "cafe\u{0301}.txt");
    let sibling = SandboxNameComparisonKey::scripted_conservative(1, "notes.txt");
    let other_parent = SandboxNameComparisonKey::scripted_conservative(2, "cafe\u{0301}.txt");

    assert!(composed == decomposed);
    assert_eq!(
        name_comparison_hash(&composed),
        name_comparison_hash(&decomposed)
    );
    assert!(composed.may_conflict_with(&decomposed));
    assert!(composed.may_conflict_with(&sibling));
    assert!(!composed.may_conflict_with(&other_parent));
}
