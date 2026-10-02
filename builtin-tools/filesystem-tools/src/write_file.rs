use crate::{FilesystemToolError, io_error, validate_path};
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, WireSchema, tool_definition, tool_implementation,
};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub enum WriteDisposition {
    Created,
    Replaced,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct WriteFileResult {
    /// Whether the call created a new file or replaced an existing file.
    pub disposition: WriteDisposition,
    /// Number of UTF-8 bytes written.
    pub bytes_written: u64,
}

#[tool_definition(version = "0.4.0", requires_filesystem = true)]
pub trait WriteFile {
    /// Creates or replaces a known UTF-8 text file and reports which occurred and the byte count.
    /// The caller must supply the path; this tool does not discover files. Errors identify unsafe
    /// paths, non-file destinations, and filesystem failures.
    #[command(annotations(
        read_only = false,
        destructive = true,
        idempotent = true,
        open_world = false
    ))]
    fn write_file(
        &self,
        path: String,
        content: String,
        create_parent_directories: bool,
    ) -> Result<WriteFileResult, FilesystemToolError>;
}

struct WriteFileImpl;

#[tool_implementation]
impl WriteFile for WriteFileImpl {
    fn write_file(
        &self,
        path: String,
        content: String,
        create_parent_directories: bool,
    ) -> Result<WriteFileResult, FilesystemToolError> {
        validate_path(&path)?;
        let file_path = Path::new(&path);
        let disposition = match fs::metadata(file_path) {
            Ok(metadata) if metadata.is_file() => WriteDisposition::Replaced,
            Ok(_) => return Err(FilesystemToolError::NotAFile(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => WriteDisposition::Created,
            Err(error) => return Err(io_error(&path, error)),
        };
        if create_parent_directories
            && let Some(parent) = file_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|error| io_error(&path, error))?;
        }
        fs::write(file_path, content.as_bytes()).map_err(|error| io_error(&path, error))?;
        Ok(WriteFileResult {
            disposition,
            bytes_written: content.len() as u64,
        })
    }
}
