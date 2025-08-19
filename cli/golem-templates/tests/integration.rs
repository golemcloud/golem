// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

#[cfg(test)]
test_r::enable!();

#[cfg(test)]
mod app {
    use assert2::{assert, let_assert};

    use std::process::Command;
    use test_r::test;

    #[test]
    #[ignore] // TODO: TEMPORARILY IGNORED UNTIL THE AGENT SDK IS SYNCED WITH THE CHANGES
    fn app_with_all_lang() {
        let status = Command::new("../../target/debug/golem-templates-test-cli")
            .args(["app"])
            .status();
        let_assert!(Ok(status) = status);
        assert!(status.success());
    }
}
