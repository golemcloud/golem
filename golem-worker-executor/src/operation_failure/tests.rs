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
fn task_failures_are_not_io_errors() {
    let failure = OperationFailure::Task(BlockingTaskError::Panicked);
    assert!(failure.io_error().is_none());
    assert!(matches!(
        failure.task_error(),
        Some(BlockingTaskError::Panicked)
    ));
    assert!(failure.cause().unwrap().is::<BlockingTaskError>());
}

#[test]
fn operation_context_formats_paths_and_preserves_typed_causes_without_policy() {
    let context = OperationContext::verification("inspect tree", Some(Path::new("/root")));
    assert_eq!(context.to_string(), "failed to inspect tree at /root");
    assert!(std::error::Error::source(&context).is_none());
    let context = OperationContext::verification("validate capacity", None);
    assert_eq!(context.to_string(), "failed to validate capacity");
    let context =
        OperationContext::task_failure("observe volume", None, BlockingTaskError::Panicked);
    assert!(context.io_kind().is_none());
    assert!(matches!(
        context.task_error(),
        Some(BlockingTaskError::Panicked)
    ));
    assert!(
        std::error::Error::source(&context)
            .unwrap()
            .is::<BlockingTaskError>()
    );
    let context = OperationContext::io(
        "read tree",
        Some(Path::new("/root")),
        std::io::ErrorKind::PermissionDenied.into(),
    );
    assert_eq!(
        context.io_kind(),
        Some(std::io::ErrorKind::PermissionDenied)
    );
    assert!(
        std::error::Error::source(&context)
            .unwrap()
            .is::<std::io::Error>()
    );
}

#[test]
fn removal_conversion_preserves_the_cause_without_adding_cleanup_policy() {
    let failure = OperationFailure::from(RemoveFailure::StillPresent);
    assert!(matches!(failure, OperationFailure::Verification));
    let failure = OperationFailure::from(RemoveFailure::Io(
        std::io::ErrorKind::PermissionDenied.into(),
    ));
    assert_eq!(
        failure.io_error().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
}
