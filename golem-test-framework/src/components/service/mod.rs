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

use std::collections::HashMap;

use tracing::Level;

pub mod spawned;

pub trait Service {
    fn kill(&self);
}

fn env_vars(env_vars: HashMap<String, String>, verbosity: Level) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();
    let vars: &[(&str, &str)] = &[
        (
            "RUST_LOG",
            &format!("{log_level},h2=warn,hyper=warn,tower=warn"),
        ),
        ("RUST_BACKTRACE", "1"),
    ];

    let mut vars: HashMap<String, String> =
        HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    vars.extend(env_vars);
    vars
}
