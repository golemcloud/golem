// Copyright 2024 Golem Cloud
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

use crate::printer::NewLine;
use crate::toml::printer::{unit, StringContext};

pub fn gen(name: &str, version: &str) -> String {
    #[rustfmt::skip]
    let code = unit() +
        "[package]" + NewLine +
        r#"name = ""# + name + r#"""# + NewLine +
        r#"version = ""# + version + r#"""# + NewLine +
        r#"edition = "2021""# + NewLine +
        r#"license = "Apache-2.0""# + NewLine +
        r#"description = "Client for Golem Cloud's REST API""# + NewLine +
        NewLine +
        "[lib]" + NewLine +
        NewLine +
        "[dependencies]" + NewLine +
        r#"async-trait = "^0.1""# + NewLine +
        r#"bytes = "^1.10""# + NewLine +
        r#"chrono = { version = "^0.4", features = ["serde"] }"# + NewLine +
        r#"futures-core = "^0.3""# + NewLine +
        r#"http = "^1.0""# + NewLine +
        r#"reqwest = { version = "^0.12", features = ["gzip", "json", "multipart", "stream"] }"# + NewLine +
        r#"serde = { version = "^1.0", features = ["derive"] }"# + NewLine +
        r#"serde_json = "^1.0""# + NewLine +
        r#"thiserror = "^2"# + NewLine +
        r#"tracing = "^0.1""# + NewLine +
        r#"uuid = { version = "^1.15", features = ["serde"] }"# + NewLine;

    StringContext::new().print_to_string(code)
}
