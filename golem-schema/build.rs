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

#[cfg(feature = "protobuf")]
fn main() -> miette::Result<()> {
    println!("cargo::rerun-if-changed=proto");

    let file_descriptors = protox::compile(
        [
            "proto/golem/common/empty.proto",
            "proto/golem/common/uuid.proto",
            "proto/golem/schema/schema.proto",
            "proto/golem/tool/tool.proto",
        ],
        ["proto"],
    )?;

    prost_build::Config::new()
        .include_file("mod.rs")
        .compile_fds(file_descriptors)
        .map_err(|err| miette::miette!(err))?;

    Ok(())
}

#[cfg(not(feature = "protobuf"))]
fn main() {}
