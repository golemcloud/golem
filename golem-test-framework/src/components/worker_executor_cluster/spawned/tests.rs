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
fn supported_cluster_preflight() {
    use std::collections::HashMap;
    let executable = std::env::current_exe().unwrap();
    let env = |http: &str, grpc: &str| {
        HashMap::from([
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__HTTP_PORT".into(),
                http.into(),
            ),
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT".into(),
                grpc.into(),
            ),
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__HOST".into(),
                "localhost".into(),
            ),
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__PORT".into(),
                "19020".into(),
            ),
        ])
    };
    let launch = |env| {
        SupportedServiceLaunch::new(
            "worker_executor",
            &executable,
            executable.parent().unwrap().into(),
            env,
        )
    };
    preflight_supported_launches(
        &[launch(env("18301", "19301")), launch(env("18413", "19417"))],
        "localhost",
        19020,
    );
    [
        ("18302", "19301"),
        ("18301", "19302"),
        ("19301", "19302"),
        ("18302", "18301"),
        ("18302", "18302"),
    ]
    .into_iter()
    .for_each(|(http, grpc)| {
        assert!(
            std::panic::catch_unwind(|| preflight_supported_launches(
                &[launch(env("18301", "19301")), launch(env(http, grpc))],
                "localhost",
                19020
            ))
            .is_err()
        );
    });
    [
        ("HTTP_PORT", None),
        ("GRPC__PORT", None),
        ("SHARD_MANAGER__HOST", None),
        ("SHARD_MANAGER__PORT", None),
        ("SHARD_MANAGER__HOST", Some("127.0.0.2")),
        ("SHARD_MANAGER__PORT", Some("19021")),
    ]
    .into_iter()
    .for_each(|(leaf, value)| {
        let mut second = env("18302", "19302");
        let key = format!("GOLEM_ADVANCED__WORKER_EXECUTOR__{leaf}");
        match value {
            Some(value) => {
                second.insert(key, value.into());
            }
            None => {
                second.remove(&key);
            }
        }
        assert!(
            std::panic::catch_unwind(|| preflight_supported_launches(
                &[launch(env("18301", "19301")), launch(second)],
                "localhost",
                19020
            ))
            .is_err()
        );
    });
    let wrong = SupportedServiceLaunch::new(
        "registry_service",
        &executable,
        executable.parent().unwrap().into(),
        HashMap::from([
            (
                "GOLEM_ADVANCED__REGISTRY_SERVICE__HTTP_PORT".into(),
                "18302".into(),
            ),
            (
                "GOLEM_ADVANCED__REGISTRY_SERVICE__GRPC__PORT".into(),
                "19302".into(),
            ),
        ]),
    );
    assert!(
        std::panic::catch_unwind(|| preflight_supported_launches(
            &[launch(env("18301", "19301")), wrong],
            "localhost",
            19020
        ))
        .is_err()
    );
}
