mod cargo;
mod rust;
mod stub;
mod wit;

use crate::cargo::generate_cargo_toml;
use crate::rust::generate_stub_source;
use crate::stub::StubDefinition;
use crate::wit::{copy_wit_files, generate_stub_wit};
use std::fs;
use std::path::Path;

fn main() {
    // TODO: inputs from clap
    let root_path = Path::new("wasm-rpc-stubgen/example");
    let dest_root = Path::new("tmp/stubgen_out");
    let selected_world = Some("api");
    let stub_crate_version = "0.0.1";
    // ^^^

    let stub_def =
        StubDefinition::new(root_path, dest_root, selected_world, stub_crate_version).unwrap();

    generate_stub_wit(&stub_def).unwrap();
    copy_wit_files(&stub_def).unwrap();

    stub_def.verify_target_wits().unwrap();

    generate_cargo_toml(&stub_def).unwrap();

    let dest_src_root = dest_root.join(Path::new("src"));
    fs::create_dir_all(dest_src_root).unwrap();
    generate_stub_source(&stub_def).unwrap();
}
