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

//! The image of the executor must give rustic a time zone. rustic asks for the time zone of the
//! system for each timestamp that a save writes, and it logs a warning when it finds none.

use test_r::test;

const DOCKERFILE: &str = include_str!("../../docker/Dockerfile");

#[test]
fn the_executor_image_sets_a_posix_utc_time_zone() {
    let final_stage = DOCKERFILE
        .rsplit_once("\nFROM ")
        .map(|(_, stage)| stage)
        .unwrap_or_default();

    assert!(
        final_stage.lines().any(|line| line.trim() == "ENV TZ=UTC0"),
        "the final stage of the executor image does not set `ENV TZ=UTC0`:\n{final_stage}"
    );
}
