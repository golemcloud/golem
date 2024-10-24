use std::path::{Path, PathBuf};

use async_zip::tokio::read::seek::ZipFileReader;
use serde::{Deserialize, Serialize};

use crate::model::FileSystemPermission;

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
    
}