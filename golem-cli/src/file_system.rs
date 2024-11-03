use golem_common::file_system::{InitialFile, InitialFileSet};
use serde::{Deserialize, Serialize};

use crate::model::{oam::{Application, TypedComponentProperties}, ComponentName, GolemError};

#[derive(Debug, Serialize, Deserialize)]
struct Properties {
    files: Vec<InitialFile>,
}

const OAM_COMPONENT_TYPE_WASM: &str = "wasm";

impl TypedComponentProperties for Properties {
    fn component_type() -> &'static str {
        OAM_COMPONENT_TYPE_WASM
    }
}

pub fn lookup_initial_files(golem_manifest: Option<&Application>, component_name: &ComponentName) -> Result<InitialFileSet, GolemError> {
    if let Some(golem_manifest) = golem_manifest {
        let mut matching_components = golem_manifest
            .spec
            .components
            .iter()
            .filter(|c| c.name == component_name.0);
    
        match (matching_components.next(), matching_components.next()) {
            (Some(component), None) => {
                let files = component.typed_properties::<Properties>()
                    .map(|p| p.files)
                    .unwrap_or_default();
    
                Ok(InitialFileSet { files })
            }
            (Some(_), Some(_)) => 
                Err(GolemError(format!("Multiple components with name '{component_name}' present in manifest"))),
            (None, _) => Err(GolemError(format!("Component '{component_name}' is not present in manifest"))),
        }
    } else {
        Ok(InitialFileSet { files: vec![] })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use test_r::test;
    use crate::model::ComponentName;

    const MANIFEST0: &str = r#"
        apiVersion: core.oam.dev/v1alpha1
        kind: ApplicationConfiguration
        metadata:
          name: test-app
        spec:
          components:
          - name: instance0
            type: wasm
            properties:
              files:
              - sourcePath: "C:\\my files\\file.txt"
                targetPath: "/my files/file.txt"
              - sourcePath: "~/"
                targetPath: "/user/"
                permission: readWrite
              - sourcePath: "example.com/internet.zip"
                targetPath: "/internet/internet.zip"
          - name: instance1
            type: wasm
            properties:
              files:
              - sourcePath: "C:\\my files\\"
                targetPath: "/my files/"
                permission: readWrite
              - sourcePath: "/file"
                targetPath: "/file"
                permission: readOnly
          - name: instance2
            type: wasm
            properties: {}
    "#;

    #[test]
    fn parse_manifest() {
        let manifest = serde_yaml::from_str(MANIFEST0).unwrap();
        let files0 = lookup_initial_files(Some(&manifest), &ComponentName("instance0".to_string())).unwrap();
        let files1 = lookup_initial_files(Some(&manifest), &ComponentName("instance1".to_string())).unwrap();
        let files2 = lookup_initial_files(Some(&manifest), &ComponentName("instance2".to_string())).unwrap();

        assert_eq!(files0.files.len(), 3);
        assert_eq!(files1.files.len(), 2);
        assert!(files2.files.is_empty());
    }

    const MANIFEST1: &str = r#"
        apiVersion: core.oam.dev/v1alpha1
        kind: ApplicationConfiguration
        metadata:
          name: test-app
        spec:
          components:
          - name: instance0
            type: wasm
            properties:
              files:
              - sourcePath: "./tests/"
                targetPath: "/src/"
                permission: readWrite
              - sourcePath: "./Cargo.toml"
                targetPath: "/src/Cargo.toml"
                permission: readWrite
              - sourcePath: "https://raw.githubusercontent.com/golemcloud/golem/refs/heads/main/golem-logo-black.jpg"
                targetPath: "/static/logo.jpg"
    "#;

    #[test]
    async fn package_files() {
        let manifest = serde_yaml::from_str(MANIFEST1).unwrap();
        let files = lookup_initial_files(Some(&manifest), &ComponentName("instance0".to_string())).unwrap();
        let (files_ro, files_rw) = files.package(None).await.unwrap().split();
        
        let files_ro = files_ro.unwrap();
        let reader_ro = async_zip::base::read::mem::ZipFileReader::new(files_ro.into_vec()).await.unwrap();

        let mut found_logo = false;
        for entry in reader_ro.file().entries() {
            println!("{}", entry.filename().as_str().unwrap());
            if entry.filename().as_str().unwrap() == "static/logo.jpg" {
                found_logo = true;
            }
        }
        assert!(found_logo);

        let files_rw = files_rw.unwrap();
        let reader_rw = async_zip::base::read::mem::ZipFileReader::new(files_rw.into_vec()).await.unwrap();

        let mut rw_count = 0;
        for _ in reader_rw.file().entries() {
            rw_count += 1;
        }
        assert!(rw_count > 1);
    }
}
