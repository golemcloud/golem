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
fn only_enabled_versioning_and_a_successful_version_check_permit_placement() {
    [
        BucketVersioning::Unversioned,
        BucketVersioning::Suspended,
        BucketVersioning::Unknown("future".into()),
    ]
    .into_iter()
    .for_each(|versioning| {
        assert_eq!(
            crate::volume::memory::place(PlacementObservations {
                versioning: Ok(versioning),
                version_read: Ok(VersionRead::NoObject),
                lifecycle: Ok(Box::new([])),
            }),
            Err(VolumeError::Requirement {
                requirement: "bucket versioning enabled".into()
            })
        );
    });
    [VersionRead::NoObject, VersionRead::Verified]
        .into_iter()
        .for_each(|version_read| {
            let lifecycle = Err(VolumeError::Denied {
                action: "GetLifecycleConfiguration".into(),
            });
            assert_eq!(
                verdict(PlacementObservations {
                    versioning: Ok(BucketVersioning::Enabled),
                    version_read: Ok(version_read),
                    lifecycle: lifecycle.clone(),
                }),
                Ok(Placement {
                    version_read,
                    lifecycle
                })
            );
        });
    [
        VolumeError::Denied {
            action: "GetBucketVersioning".into(),
        },
        VolumeError::Unanswered,
    ]
    .into_iter()
    .for_each(|error| {
        assert_eq!(
            verdict(PlacementObservations {
                versioning: Err(error.clone()),
                version_read: Ok(VersionRead::NoObject),
                lifecycle: Ok(Box::new([])),
            }),
            Err(error)
        );
    });
    let denied = VolumeError::Denied {
        action: "GetObjectVersion".into(),
    };
    assert_eq!(
        verdict(PlacementObservations {
            versioning: Ok(BucketVersioning::Enabled),
            version_read: Err(denied.clone()),
            lifecycle: Ok(Box::new([])),
        }),
        Err(denied)
    );
    let rules: Box<[_]> = [LifecycleRule {
        status: LifecycleRuleStatus::Enabled,
        prefix: Some("corpus/".into()),
        noncurrent_expiration: Some(NoncurrentExpiration {
            days: 17,
            newer_versions: Some(5),
        }),
    }]
    .into();
    assert_eq!(
        verdict(PlacementObservations {
            versioning: Ok(BucketVersioning::Enabled),
            version_read: Ok(VersionRead::Verified),
            lifecycle: Ok(rules.clone()),
        }),
        Ok(Placement {
            version_read: VersionRead::Verified,
            lifecycle: Ok(rules)
        })
    );
}
