// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[cfg(test)]
test_r::enable!();

#[cfg(test)]
mod templates {
    use assert2::{assert, let_assert};
    use std::process::Command;
    use test_r::test;

    #[test]
    fn templates_c() {
        test_templates("c")
    }

    #[test]
    fn templates_go() {
        test_templates("go")
    }

    #[test]
    fn templates_js() {
        test_templates("js")
    }

    #[test]
    fn templates_python() {
        test_templates("python")
    }

    #[test]
    fn templates_rust() {
        test_templates("rust")
    }

    #[test]
    fn templates_ts() {
        test_templates("ts")
    }

    #[test]
    fn templates_zig() {
        test_templates("zig")
    }

    fn test_templates(test_prefix: &str) {
        let status = Command::new("../target/debug/golem-templates-test-cli")
            .args([
                "templates",
                "--filter",
                &format!("^{}-", test_prefix),
                "--target-path",
                "../target/templates-test",
            ])
            .status();
        let_assert!(Ok(status) = status);
        assert!(status.success());
    }
}

#[cfg(test)]
mod app {
    use assert2::{assert, let_assert};

    use std::process::Command;
    use test_r::test;

    #[test]
    fn app_with_all_lang() {
        let status = Command::new("../target/debug/golem-templates-test-cli")
            .args(["app"])
            .status();
        let_assert!(Ok(status) = status);
        assert!(status.success());
    }
}
