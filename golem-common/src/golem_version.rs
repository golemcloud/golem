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

pub use git_version::git_version;

#[macro_export]
macro_rules! golem_version {
    () => {{
        let version = golem_common::golem_version::git_version!(
            args = ["--tags"],
            cargo_prefix = "",
            fallback = "0.0.0"
        );
        if !version.is_empty() && version.as_bytes()[0] == b'v' {
            unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                    version.as_ptr().add(1),
                    version.len() - 1,
                ))
            }
        } else {
            version
        }
    }};
}
