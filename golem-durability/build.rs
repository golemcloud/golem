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

use std::path::Path;
use std::process::Command;

fn main() {
    let cargo_manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let cargo_manifest_dir = Path::new(&cargo_manifest_dir);
    let durable_wasi_path = cargo_manifest_dir.join("../durable-wasi");
    let profile = std::env::var("PROFILE").unwrap();

    println!("cargo:warning=Building durable-wasi component at {durable_wasi_path:?} with {profile} profile");

    if profile == "release" {
        Command::new("cargo")
            .args(["component", "build", "--release"])
            .current_dir(durable_wasi_path.clone())
            .status()
            .unwrap();
    } else {
        Command::new("cargo")
            .args(["component", "build"])
            .current_dir(durable_wasi_path.clone())
            .status()
            .unwrap();
    };

    let source_path = durable_wasi_path
        .join("target")
        .join("wasm32-wasip1")
        .join(profile)
        .join("durable_wasi.wasm");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_dir = Path::new(&out_dir);
    let target_path = out_dir.join("durable_wasi.wasm");

    println!(
        "cargo:warning=Copying durable-wasi component from {source_path:?} to {target_path:?}"
    );

    std::fs::copy(source_path, target_path.clone()).unwrap();

    println!(
        "cargo::rustc-env=DURABLE_WASI_COMPONENT={}",
        target_path.to_str().unwrap()
    );
}
