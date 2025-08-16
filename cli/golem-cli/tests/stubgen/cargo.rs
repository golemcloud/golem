use crate::stubgen::{cargo_component_build, test_data_path};
use assert2::check;
use fs_extra::dir::CopyOptions;
use golem_cli::fs;
use golem_cli::wasm_rpc_stubgen::cargo::regenerate_cargo_package_component;
use tempfile::TempDir;
use test_r::test;

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
    check!(cargo_toml.contains("\n# Hello\nwit-bindgen-rt = \"0.40.0\""));
    check!(cargo_toml.contains("\n# This is the comment for lib\n[lib]"));
    check!(cargo_toml.contains("\n# Another comment\ncrate-type = [\"cdylib\"] # Hello again"));
    check!(!cargo_toml.contains("[package.metadata.component.target]\npath = \"wit\""));
    check!(!cargo_toml.contains("[package.metadata.component.target.dependencies]"));
    check!(!cargo_toml.contains("\"test:sub\" = { path = \"wit/deps/sub\" }"));
    check!(!cargo_toml.contains("\"test:sub2\" = { path = \"wit/deps/sub2\" }"));

    // Regenerate and check for comments and deps
    regenerate_cargo_package_component(&cargo_toml_path, &wit_path, None).unwrap();
    let cargo_toml = fs::read_to_string(&cargo_toml_path).unwrap();
    println!(">\n{cargo_toml}");
    cargo_component_build(project_dir.path());
    check!(cargo_toml.contains("\n# Hello\nwit-bindgen-rt = \"0.40.0\""));
    check!(cargo_toml.contains("\n# This is the comment for lib\n[lib]"));
    check!(cargo_toml.contains("\n# Another comment\ncrate-type = [\"cdylib\"] # Hello again"));
    check!(cargo_toml.contains("[package.metadata.component.target]\npath = \"wit\""));
    check!(cargo_toml.contains("[package.metadata.component.target.dependencies]"));
    check!(cargo_toml.contains("\"test:sub\" = { path = \"wit/deps/sub\" }"));
    check!(cargo_toml.contains("\"test:sub2\" = { path = \"wit/deps/sub2\" }"));

    // Regenerate again and check for comments and deps
    regenerate_cargo_package_component(&cargo_toml_path, &wit_path, None).unwrap();
    let cargo_toml = fs::read_to_string(&cargo_toml_path).unwrap();
    println!(">\n{cargo_toml}");
    cargo_component_build(project_dir.path());
    check!(cargo_toml.contains("\n# Hello\nwit-bindgen-rt = \"0.40.0\""));
    check!(cargo_toml.contains("\n# This is the comment for lib\n[lib]"));
    check!(cargo_toml.contains("\n# Another comment\ncrate-type = [\"cdylib\"] # Hello again"));
    check!(cargo_toml.contains("[package.metadata.component.target]\npath = \"wit\""));
    check!(cargo_toml.contains("[package.metadata.component.target.dependencies]"));
    check!(cargo_toml.contains("\"test:sub\" = { path = \"wit/deps/sub\" }"));
    check!(cargo_toml.contains("\"test:sub2\" = { path = \"wit/deps/sub2\" }"));

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
    check!(cargo_toml.contains("\n# Hello\nwit-bindgen-rt = \"0.40.0\""));
    check!(cargo_toml.contains("\n# This is the comment for lib\n[lib]"));
    check!(cargo_toml.contains("\n# Another comment\ncrate-type = [\"cdylib\"] # Hello again"));
    check!(cargo_toml.contains("[package.metadata.component.target]\npath = \"wit\""));
    check!(cargo_toml.contains("[package.metadata.component.target.dependencies]"));
    check!(!cargo_toml.contains("\"test:sub\" = { path = \"wit/deps/sub\" }"));
    check!(!cargo_toml.contains("\"test:sub2\" = { path = \"wit/deps/sub2\" }"));

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
    check!(cargo_toml.contains("\n# Hello\nwit-bindgen-rt = \"0.40.0\""));
    check!(cargo_toml.contains("\n# This is the comment for lib\n[lib]"));
    check!(cargo_toml.contains("\n# Another comment\ncrate-type = [\"cdylib\"] # Hello again"));
    check!(cargo_toml.contains("[package.metadata.component.target]\npath = \"wit\""));
    check!(cargo_toml.contains("[package.metadata.component.target.dependencies]"));
    check!(cargo_toml.contains("\"test:sub\" = { path = \"wit/deps/sub\" }"));
    check!(cargo_toml.contains("\"test:sub2\" = { path = \"wit/deps/sub2\" }"));

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
    check!(cargo_toml.contains("\n# Hello\nwit-bindgen-rt = \"0.40.0\""));
    check!(cargo_toml.contains("\n# This is the comment for lib\n[lib]"));
    check!(cargo_toml.contains("\n# Another comment\ncrate-type = [\"cdylib\"] # Hello again"));
    check!(cargo_toml.contains("[package.metadata.component.target]\npath = \"wit\""));
    check!(cargo_toml.contains("[package.metadata.component.target.dependencies]"));
    check!(cargo_toml.contains("\"test:sub\" = { path = \"wit/deps/sub\" }"));
    check!(cargo_toml.contains("[package.metadata.component.bindings]\nderives = [\"serde::Serialize\", \"serde::Deserialize\"]\ngenerate_unused_types = true"));
}
