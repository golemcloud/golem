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

use cargo_metadata::MetadataCommand;
use std::env;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let golem_wit_root = PathBuf::from(
        env::var("GOLEM_WIT_ROOT").unwrap_or_else(|_| find_package_root("golem-wit")),
    );

    println!("cargo:warning=Output dir: {out_dir:?}");
    println!("cargo:warning=Golem WIT root: {golem_wit_root:?}");

    let target = out_dir.join("golem-wit");
    let target_exists = target.exists();
    let target_is_different =
        target_exists && dir_diff::is_different(&golem_wit_root, &target).unwrap_or(true);

    if target_exists && !target_is_different {
        println!("cargo:warning=Golem WIT is up to date in {target:?}");
        return;
    }

    fs_extra::dir::create(&target, true).unwrap();
    fs_extra::dir::copy(
        &golem_wit_root,
        &target,
        &fs_extra::dir::CopyOptions::new().content_only(true),
    )
    .unwrap();

    if target_exists {
        println!("cargo:warning=Golem WIT was updated in {target:?}");
    } else {
        println!("cargo:warning=Golem WIT was created in {target:?}");
    }
}

fn find_package_root(name: &str) -> String {
    let metadata = MetadataCommand::new()
        .manifest_path("./Cargo.toml")
        .exec()
        .unwrap();
    let package = metadata.packages.iter().find(|p| p.name == name).unwrap();
    package.manifest_path.parent().unwrap().to_string()
}
