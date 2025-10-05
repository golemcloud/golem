use crate::stubgen::{cargo_component_build, test_data_path};
use assert2::check;
use fs_extra::dir::CopyOptions;
use golem_cli::fs;
use golem_cli::wasm_rpc_stubgen::cargo::regenerate_cargo_package_component;
use tempfile::TempDir;
use test_r::test;

#[cfg(not(windows))]
const WIT_BINDGEN_RT_TEXT: &str = "\n# Hello\nwit-bindgen-rt = \"0.40.0\"";
#[cfg(not(windows))]
const COMMENT_FOR_LIB_TEXT: &str = "\n# This is the comment for lib\n[lib]";
#[cfg(not(windows))]
const CRATE_TYPE_TEXT: &str = "\n# Another comment\ncrate-type = [\"cdylib\"] # Hello again";
#[cfg(not(windows))]
const COMPONENT_METADATA_TEXT: &str = "[package.metadata.component.target]\npath = \"wit\"";
#[cfg(not(windows))]
const COMPONENT_BINDINGS_TEXT: &str = "[package.metadata.component.bindings]\nderives = [\"serde::Serialize\", \"serde::Deserialize\"]\ngenerate_unused_types = true";
#[cfg(not(windows))]
const DEPENDENCY_TEST_SUB_TEXT: &str = "\"test:sub\" = { path = \"wit/deps/sub\" }";
#[cfg(not(windows))]
const DEPENDENCY_TEST_SUB_2_TEXT: &str = "\"test:sub2\" = { path = \"wit/deps/sub2\" }";

#[cfg(windows)]
const WIT_BINDGEN_RT_TEXT: &str = "\r\n# Hello\r\nwit-bindgen-rt = \"0.40.0\"";
#[cfg(windows)]
const COMMENT_FOR_LIB_TEXT: &str = "\r\n# This is the comment for lib\r\n[lib]";
#[cfg(windows)]
const CRATE_TYPE_TEXT: &str = "\r\n# Another comment\r\ncrate-type = [\"cdylib\"] # Hello again";
#[cfg(windows)]
const COMPONENT_METADATA_TEXT: &str = "[package.metadata.component.target]\r\npath = \"wit\"";
#[cfg(windows)]
const COMPONENT_BINDINGS_TEXT: &str = "[package.metadata.component.bindings]\r\nderives = [\"serde::Serialize\", \"serde::Deserialize\"]\r\ngenerate_unused_types = true";
#[cfg(windows)]
const DEPENDENCY_TEST_SUB_TEXT: &str = "\"test:sub\" = { path = 'wit\\deps\\sub' }";
#[cfg(windows)]
const DEPENDENCY_TEST_SUB_2_TEXT: &str = "\"test:sub2\" = { path = 'wit\\deps\\sub2' }";

const COMPONENT_METADATA_DEPS_TEXT: &str = "[package.metadata.component.target.dependencies]";

#[test]
fn regenerate_cargo_toml() {
    // Setup cargo project
    let project_dir = TempDir::new().unwrap();
    let cargo_toml_path = project_dir.path().join("Cargo.toml");
    let wit_path = project_dir.path().join("wit");

    fs_extra::dir::copy(
        test_data_path().join("wit").join("many-ways-to-export"),
        &wit_path,
        &CopyOptions::new().content_only(true).overwrite(true),
    )
    .unwrap();
    fs::copy(
        test_data_path()
            .join("cargo")
            .join("Cargo.toml.with_deps_and_comments"),
        &cargo_toml_path,
    )
    .unwrap();
    fs::write_str(project_dir.path().join("src").join("lib.rs"), "").unwrap();

    // Check that we have the original comments
    let cargo_toml = fs::read_to_string(&cargo_toml_path).unwrap();
    println!(">\n{cargo_toml}");
    check!(cargo_toml.contains(WIT_BINDGEN_RT_TEXT));
    check!(cargo_toml.contains(COMMENT_FOR_LIB_TEXT));
    check!(cargo_toml.contains(CRATE_TYPE_TEXT));
    check!(!cargo_toml.contains(COMPONENT_METADATA_TEXT));
    check!(!cargo_toml.contains(COMPONENT_METADATA_DEPS_TEXT));
    check!(!cargo_toml.contains(DEPENDENCY_TEST_SUB_TEXT));
    check!(!cargo_toml.contains(DEPENDENCY_TEST_SUB_2_TEXT));

    // Regenerate and check for comments and deps
    regenerate_cargo_package_component(&cargo_toml_path, &wit_path, None).unwrap();
    let cargo_toml = fs::read_to_string(&cargo_toml_path).unwrap();
    println!(">\n{cargo_toml}");
    cargo_component_build(project_dir.path());
    check!(cargo_toml.contains(WIT_BINDGEN_RT_TEXT));
    check!(cargo_toml.contains(COMMENT_FOR_LIB_TEXT));
    check!(cargo_toml.contains(CRATE_TYPE_TEXT));
    check!(cargo_toml.contains(COMPONENT_METADATA_TEXT));
    check!(cargo_toml.contains(COMPONENT_METADATA_DEPS_TEXT));
    check!(cargo_toml.contains(DEPENDENCY_TEST_SUB_TEXT));
    check!(cargo_toml.contains(DEPENDENCY_TEST_SUB_2_TEXT));

    // Regenerate again and check for comments and deps
    regenerate_cargo_package_component(&cargo_toml_path, &wit_path, None).unwrap();
    let cargo_toml = fs::read_to_string(&cargo_toml_path).unwrap();
    println!(">\n{cargo_toml}");
    cargo_component_build(project_dir.path());
    check!(cargo_toml.contains(WIT_BINDGEN_RT_TEXT));
    check!(cargo_toml.contains(COMMENT_FOR_LIB_TEXT));
    check!(cargo_toml.contains(CRATE_TYPE_TEXT));
    check!(cargo_toml.contains(COMPONENT_METADATA_TEXT));
    check!(cargo_toml.contains(COMPONENT_METADATA_DEPS_TEXT));
    check!(cargo_toml.contains(DEPENDENCY_TEST_SUB_TEXT));
    check!(cargo_toml.contains(DEPENDENCY_TEST_SUB_2_TEXT));

    // Swap wit dir to one that has no deps, regenerate and check for comments and "no deps"
    fs::remove(&wit_path).unwrap();
    fs_extra::dir::copy(
        test_data_path().join("wit").join("all-wit-types"),
        &wit_path,
        &CopyOptions::new().content_only(true).overwrite(true),
    )
    .unwrap();
    regenerate_cargo_package_component(&cargo_toml_path, &wit_path, None).unwrap();
    let cargo_toml = fs::read_to_string(&cargo_toml_path).unwrap();
    println!(">\n{cargo_toml}");
    cargo_component_build(project_dir.path());
    check!(cargo_toml.contains(WIT_BINDGEN_RT_TEXT));
    check!(cargo_toml.contains(COMMENT_FOR_LIB_TEXT));
    check!(cargo_toml.contains(CRATE_TYPE_TEXT));
    check!(cargo_toml.contains(COMPONENT_METADATA_TEXT));
    check!(cargo_toml.contains(COMPONENT_METADATA_DEPS_TEXT));
    check!(!cargo_toml.contains(DEPENDENCY_TEST_SUB_TEXT));
    check!(!cargo_toml.contains(DEPENDENCY_TEST_SUB_2_TEXT));

    // Swap wit dir back, regenerate and check for comments and deps
    fs::remove(&wit_path).unwrap();
    fs_extra::dir::copy(
        test_data_path().join("wit").join("many-ways-to-export"),
        &wit_path,
        &CopyOptions::new().content_only(true).overwrite(true),
    )
    .unwrap();
    regenerate_cargo_package_component(&cargo_toml_path, &wit_path, None).unwrap();
    let cargo_toml = fs::read_to_string(&cargo_toml_path).unwrap();
    println!(">\n{cargo_toml}");
    cargo_component_build(project_dir.path());
    check!(cargo_toml.contains(WIT_BINDGEN_RT_TEXT));
    check!(cargo_toml.contains(COMMENT_FOR_LIB_TEXT));
    check!(cargo_toml.contains(CRATE_TYPE_TEXT));
    check!(cargo_toml.contains(COMPONENT_METADATA_TEXT));
    check!(cargo_toml.contains(COMPONENT_METADATA_DEPS_TEXT));
    check!(cargo_toml.contains(DEPENDENCY_TEST_SUB_TEXT));
    check!(cargo_toml.contains(DEPENDENCY_TEST_SUB_2_TEXT));

    // Append component binding customization to Cargo.toml, then regenerate and check for all
    fs::write_str(
        &cargo_toml_path,
        fs::read_to_string(&cargo_toml_path).unwrap()
            + r#"
[package.metadata.component.bindings]
derives = ["serde::Serialize", "serde::Deserialize"]
generate_unused_types = true
"#,
    )
    .unwrap();
    regenerate_cargo_package_component(&cargo_toml_path, &wit_path, None).unwrap();
    let cargo_toml = fs::read_to_string(&cargo_toml_path).unwrap();
    println!(">\n{cargo_toml}");
    cargo_component_build(project_dir.path());
    check!(cargo_toml.contains(WIT_BINDGEN_RT_TEXT));
    check!(cargo_toml.contains(COMMENT_FOR_LIB_TEXT));
    check!(cargo_toml.contains(CRATE_TYPE_TEXT));
    check!(cargo_toml.contains(COMPONENT_METADATA_TEXT));
    check!(cargo_toml.contains(COMPONENT_METADATA_DEPS_TEXT));
    check!(cargo_toml.contains(DEPENDENCY_TEST_SUB_TEXT));
    check!(cargo_toml.contains(COMPONENT_BINDINGS_TEXT));
}
