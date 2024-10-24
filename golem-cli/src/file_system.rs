use std::{collections::HashSet, fmt::Display, path::{Path, PathBuf}};

use async_zip::{tokio::write::ZipFileWriter, Compression, ZipEntryBuilder};
use futures_util::{AsyncWriteExt as _, StreamExt as _};
use golem_common::{file_system::{InitialFile, InitialFileSet, PackagedFileSet}, model::FileSystemPermission};
use serde::{Deserialize, Serialize};
use url::Url;
use walkdir::WalkDir;

use crate::model::{oam::{Application, TypedComponentProperties}, ComponentName, GolemError};

#[derive(Debug)]
enum PathOrUrl {
    Path(PathBuf),
    Url(Url),
}

impl PathOrUrl {
    pub fn new(path_or_url: PathBuf) -> Self {
        match Url::parse(path_or_url.to_string_lossy().as_ref()) {
            Ok(url) => if url.scheme() == "file" {
                    Self::Path(path_or_url)
                } else {
                    Self::Url(url)
                },
            Err(_) => Self::Path(path_or_url),
        }
    }
}

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

pub async fn package_initial_files(initial_file_set: InitialFileSet) -> Result<PackagedFileSet, GolemError> {
    let mut targets = HashSet::new();

    let mut files_ro = vec![];
    let mut files_ro_writer = ZipFileWriter::with_tokio(&mut files_ro);
    let mut files_rw = vec![];
    let mut files_rw_writer = ZipFileWriter::with_tokio(&mut files_rw);
    
    let mut client = None;

    for file in initial_file_set.files {
        let InitialFile {
            source_path,
            target_path,
            permission,
        } = file;

        // async_zip needs a relative path
        let target_path = match target_path.strip_prefix(Path::new("/")) {
            Ok(target_path) => target_path.to_owned(),
            Err(_) => target_path,
        };

        let files_writer = match permission {
            FileSystemPermission::ReadOnly => &mut files_ro_writer,
            FileSystemPermission::ReadWrite => &mut files_rw_writer,
        };

        struct ErrorContext<C>(C);
        impl<C: Display> ErrorContext<C> {
            fn golem_error<E: Display>(&self) -> impl '_ + Fn(E) -> GolemError {
                |err| GolemError(format!("Failed to package '{}': {err}", self.0))
            }
        }

        match PathOrUrl::new(source_path) {
            PathOrUrl::Path(source_path) => {
                let context = ErrorContext(source_path.display());

                let walk = WalkDir::new(&source_path);

                for entry in walk {
                    let entry = entry.map_err(context.golem_error())?;
                    let context = ErrorContext(entry.path().display());
                    let metadata = entry.metadata()
                        .map_err(context.golem_error())?;
                    if metadata.file_type().is_file() {
                        let target_suffix = entry.path()
                            .strip_prefix(&source_path)
                            .map_err(context.golem_error())?;
                        
                        let target_path = if target_suffix == Path::new("") {
                            target_path.clone()
                        } else { 
                            target_path.join(target_suffix)
                        };

                        if let Some(conflict) = targets.replace(target_path.clone()) {
                            return Err(context.golem_error()(format!("Multiple files provided for target path '{}'", conflict.display())));
                        }

                        let zip_entry = ZipEntryBuilder::new(target_path.to_string_lossy().to_string().into(), Compression::Deflate)
                            .build();
    
                        let mut entry_writer = files_writer.write_entry_stream(zip_entry)
                            .await
                            .map_err(context.golem_error())?;
                        
                        let file_contents = tokio::fs::read(entry.path())
                            .await
                            .map_err(context.golem_error())?;

                        entry_writer.write_all(&*file_contents)
                            .await
                            .map_err(context.golem_error())?;

                        entry_writer.close()
                            .await
                            .map_err(context.golem_error())?;
                    }
                }
            }
            PathOrUrl::Url(source_url) => {
                let context = ErrorContext(&source_url);

                if let Some(conflict) = targets.replace(target_path.clone()) {
                    return Err(context.golem_error()(format!("Multiple files provided for target path '{}'", conflict.display())));
                }

                let entry = ZipEntryBuilder::new(target_path.to_string_lossy().to_string().into(), Compression::Deflate)
                    .build();

                let mut entry_writer = files_writer.write_entry_stream(entry)
                    .await
                    .map_err(context.golem_error())?;

                let client = client.get_or_insert_with(reqwest::Client::new);
                
                let response = client.get(source_url.clone())
                    .send()
                    .await
                    .map_err(context.golem_error())?
                    .error_for_status()
                    .map_err(context.golem_error())?;

                let mut response_stream = response.bytes_stream();
                while let Some(chunk) = response_stream.next().await {
                    let chunk = chunk
                        .map_err(context.golem_error())?;
                    entry_writer.write_all(chunk.as_ref())
                        .await
                        .map_err(context.golem_error())?;
                }

                entry_writer.close()
                    .await
                    .map_err(context.golem_error())?;
            }
        }
    }

    files_ro_writer.close().await
        .map_err(|e| GolemError(format!("Failed to archive read-only files: {e}")))?;

    files_rw_writer.close().await
        .map_err(|e| GolemError(format!("Failed to archive read-write files: {e}")))?;

    Ok(PackagedFileSet::from_vecs(files_ro, files_rw))
}

#[cfg(test)]
mod test {
    use crate::model::ComponentName;

    use super::*;

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

    #[tokio::test]
    async fn package_files() {
        let manifest = serde_yaml::from_str(MANIFEST1).unwrap();
        let files = lookup_initial_files(Some(&manifest), &ComponentName("instance0".to_string())).unwrap();
        let (files_ro, files_rw) = package_initial_files(files).await.unwrap().split();
        
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
