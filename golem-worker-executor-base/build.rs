use cargo_metadata::MetadataCommand;
use std::path::Path;
use symlink::{remove_symlink_dir, symlink_dir};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let golem_wit_root = find_package_root("golem-wit");
    let target_root = Path::new("./golem-wit");
    if target_root.exists() {
        let _ = remove_symlink_dir(target_root);
    }
    println!(
        "Creating symlink from {} to {}",
        golem_wit_root,
        target_root.display()
    );
    symlink_dir(golem_wit_root, target_root)?;
    Ok(())
}

fn find_package_root(name: &str) -> String {
    let metadata = MetadataCommand::new()
        .manifest_path("./Cargo.toml")
        .exec()
        .unwrap();
    let package = metadata.packages.iter().find(|p| p.name == name).unwrap();
    package.manifest_path.parent().unwrap().to_string()
}
