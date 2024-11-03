use std::{collections::HashSet, fmt::Display, path::{Path, PathBuf}};

use async_zip::{tokio::{read::seek::ZipFileReader, write::ZipFileWriter}, Compression, ZipEntryBuilder};
use serde::{Deserialize, Serialize};
use futures_util::{AsyncWriteExt as _, StreamExt as _};
use url::Url;
use walkdir::WalkDir;

use crate::model::FileSystemPermission;

/// An in-memory file archive. Currently a Deflate zip file
#[derive(Clone)]
pub struct PackagedFiles {
    data: Vec<u8>,
}

impl PackagedFiles {
    pub fn from_vec(data: Vec<u8>) -> Option<Self> {
        if data.is_empty() {
            None
        } else {
            Some(Self {
                data,
            })
        }
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }

    pub async fn extract(&self, target_path: &Path) -> std::io::Result<()> {
        let target_dir = target_path.canonicalize()?;

        let mut zip_reader =  ZipFileReader::with_tokio(std::io::Cursor::new(self.data.as_slice()))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let entry_count = zip_reader.file().entries().len();
        for index in 0..entry_count {
            let mut entry_reader = zip_reader.reader_with_entry(index)
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

            let filename = entry_reader.entry().filename()
                .as_str()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

            let target_path = target_dir.join(filename);
            // Don't allow paths that would escape our target_dir
            let _ = target_path.strip_prefix(&target_dir)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
            
            let mut buf = vec![];
            entry_reader.read_to_end_checked(&mut buf)
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

            if let Some(target_path_dir) = target_path.parent() {
                tokio::fs::create_dir_all(target_path_dir)
                    .await?;
            }

            tokio::fs::write(target_path, buf)
                .await?;
        }

        Ok(())
    }
}

impl std::fmt::Debug for PackagedFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackagedFiles")
            .field("data", &format!("[{} bytes]", self.data.len()))
            .finish()
    }
}

/// A pair of `PackagedFiles`, one with read-only files, 
/// the other with read-write. Both are optional.
#[derive(Debug, Clone)]
pub struct PackagedFileSet {
    files_ro: Option<PackagedFiles>,
    files_rw: Option<PackagedFiles>,
}

impl PackagedFileSet {
    pub fn empty() -> Self {
        Self {
            files_ro: None,
            files_rw: None,
        }
    }

    pub fn from_vecs(files_ro: Vec<u8>, files_rw: Vec<u8>) -> Self {
        Self {
            files_ro: PackagedFiles::from_vec(files_ro),
            files_rw: PackagedFiles::from_vec(files_rw),
        }
    }

    pub fn split(self) -> (Option<PackagedFiles>, Option<PackagedFiles>) {
        (self.files_ro, self.files_rw)
    }

    pub fn split_vec(self) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        (
            self.files_ro.map(PackagedFiles::into_vec), 
            self.files_rw.map(PackagedFiles::into_vec),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialFile {
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    #[serde(default)]
    pub permission: FileSystemPermission,
}

#[derive(Debug, Default)]
pub struct InitialFileSet {
    pub files: Vec<InitialFile>,
}

impl InitialFileSet {
    pub async fn package(&self, working_directory: Option<&Path>) -> Result<PackagedFileSet, String> {
        let manifest_directory = working_directory.unwrap_or(Path::new("."));
    
        let mut targets = HashSet::new();
    
        let mut files_ro = vec![];
        let mut files_ro_writer = ZipFileWriter::with_tokio(&mut files_ro);
        let mut files_rw = vec![];
        let mut files_rw_writer = ZipFileWriter::with_tokio(&mut files_rw);
        
        let mut client = None;
    
        for file in &self.files {
            let InitialFile {
                source_path,
                target_path,
                permission,
            } = file;
    
            // async_zip needs a relative path
            let target_path = match target_path.strip_prefix(Path::new("/")) {
                Ok(target_path) => target_path,
                Err(_) => target_path,
            };
    
            let files_writer = match permission {
                FileSystemPermission::ReadOnly => &mut files_ro_writer,
                FileSystemPermission::ReadWrite => &mut files_rw_writer,
            };
    
            struct ErrorContext<C>(C);
            impl<C: Display> ErrorContext<C> {
                fn to_error<E: Display>(&self) -> impl '_ + Fn(E) -> String {
                    |err| format!("Failed to package '{}': {err}", self.0)
                }
            }
    
            match PathOrUrl::new(source_path) {
                PathOrUrl::Path(source_path) => {
                    let context = ErrorContext(source_path.display());
                    let source_path = manifest_directory.join(&source_path);
    
                    let walk = WalkDir::new(&source_path);
    
                    for entry in walk {
                        let entry = entry.map_err(context.to_error())?;
                        let context = ErrorContext(entry.path().display());
                        let metadata = entry.metadata()
                            .map_err(context.to_error())?;
                        if metadata.file_type().is_file() {
                            let target_suffix = entry.path()
                                .strip_prefix(&source_path)
                                .map_err(context.to_error())?;
                            
                            let target_path = if target_suffix == Path::new("") {
                                target_path.to_path_buf()
                            } else { 
                                target_path.join(target_suffix)
                            };
    
                            if let Some(conflict) = targets.replace(target_path.clone()) {
                                return Err(context.to_error()(format!("Multiple files provided for target path '{}'", conflict.display())));
                            }
    
                            let zip_entry = ZipEntryBuilder::new(target_path.to_string_lossy().to_string().into(), Compression::Deflate)
                                .build();
        
                            let mut entry_writer = files_writer.write_entry_stream(zip_entry)
                                .await
                                .map_err(context.to_error())?;
                            
                            let file_contents = tokio::fs::read(entry.path())
                                .await
                                .map_err(context.to_error())?;
                            
                            entry_writer.write_all(&*file_contents)
                                .await
                                .map_err(context.to_error())?;
    
                            entry_writer.close()
                                .await
                                .map_err(context.to_error())?;
                        }
                    }
                }
                PathOrUrl::Url(source_url) => {
                    let context = ErrorContext(&source_url);
    
                    if let Some(conflict) = targets.replace(target_path.to_path_buf()) {
                        return Err(context.to_error()(format!("Multiple files provided for target path '{}'", conflict.display())));
                    }
    
                    let entry = ZipEntryBuilder::new(target_path.to_string_lossy().to_string().into(), Compression::Deflate)
                        .build();
    
                    let mut entry_writer = files_writer.write_entry_stream(entry)
                        .await
                        .map_err(context.to_error())?;
    
                    let client = client.get_or_insert_with(reqwest::Client::new);
                    
                    let response = client.get(source_url.clone())
                        .send()
                        .await
                        .map_err(context.to_error())?
                        .error_for_status()
                        .map_err(context.to_error())?;
    
                    let mut response_stream = response.bytes_stream();
                    while let Some(chunk) = response_stream.next().await {
                        let chunk = chunk
                            .map_err(context.to_error())?;
                        entry_writer.write_all(chunk.as_ref())
                            .await
                            .map_err(context.to_error())?;
                    }
    
                    entry_writer.close()
                        .await
                        .map_err(context.to_error())?;
                }
            }
        }
    
        files_ro_writer.close().await
            .map_err(|e| format!("Failed to archive read-only files: {e}"))?;
    
        files_rw_writer.close().await
            .map_err(|e| format!("Failed to archive read-write files: {e}"))?;
    
        Ok(PackagedFileSet::from_vecs(files_ro, files_rw))
    }
}


#[derive(Debug)]
enum PathOrUrl<'p> {
    Path(&'p Path),
    Url(Url),
}

impl<'p> PathOrUrl<'p> {
    pub fn new(path_or_url: &'p Path) -> Self {
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

pub const READ_ONLY_FILES_PATH: &str = "static";
