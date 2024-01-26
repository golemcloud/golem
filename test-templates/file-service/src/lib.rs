cargo_component_bindings::generate!();

use std::fs;
use crate::bindings::exports::golem::it::api::{Datetime, FileInfo, Guest};
use crate::bindings::wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};
use crate::bindings::wasi::filesystem::preopens::get_directories;

use std::fs::{File, read_to_string, remove_file, write};

struct Component;

impl Guest for Component {
    fn read_file(path: String) -> Result<String, String> {
        read_to_string(path).map_err(|e| e.to_string())
    }

    fn write_file(path: String, contents: String) -> Result<(), String> {
        // this uses write-via-stream internally
        write(path, contents).map_err(|e| e.to_string())
    }

    fn delete_file(path: String) -> Result<(), String> {
        remove_file(path).map_err(|e| e.to_string())
    }

    fn write_file_direct(name: String, contents: String) -> Result<(), String> {
        // Directly using the filesystem API to call Descriptor::write
        let (root, _) = get_directories().into_iter().find(|(_, path)| path == "/").ok_or("Root not found")?;
        let file = root.open_at(PathFlags::empty(), &name, OpenFlags::CREATE, DescriptorFlags::WRITE).map_err(|_| "Failed to open file")?;
        let _ = file.write(contents.as_bytes(), 0).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn get_file_info(path: String) -> Result<FileInfo, String> {
        let file = File::open(path).map_err(|err| err.to_string())?;
        let metadata = file.metadata().map_err(|err| err.to_string())?;
        let modified = metadata.modified().map_err(|err| err.to_string())?.duration_since(std::time::UNIX_EPOCH).map_err(|err| err.to_string())?;
        let accessed = metadata.accessed().map_err(|err| err.to_string())?.duration_since(std::time::UNIX_EPOCH).map_err(|err| err.to_string())?;
        let last_modified = Datetime {
            seconds: modified.as_secs(),
            nanoseconds: modified.subsec_nanos(),
        };
        let last_accessed = Datetime {
            seconds: accessed.as_secs(),
            nanoseconds: accessed.subsec_nanos(),
        };
        Ok(FileInfo {
            last_modified,
            last_accessed,
        })
    }

    fn get_info(path: String) -> Result<FileInfo, String> {
        let metadata = fs::symlink_metadata(path).map_err(|err| err.to_string())?;
        let modified = metadata.modified().map_err(|err| err.to_string())?.duration_since(std::time::UNIX_EPOCH).map_err(|err| err.to_string())?;
        let accessed = metadata.accessed().map_err(|err| err.to_string())?.duration_since(std::time::UNIX_EPOCH).map_err(|err| err.to_string())?;
        let last_modified = Datetime {
            seconds: modified.as_secs(),
            nanoseconds: modified.subsec_nanos(),
        };
        let last_accessed = Datetime {
            seconds: accessed.as_secs(),
            nanoseconds: accessed.subsec_nanos(),
        };
        Ok(FileInfo {
            last_modified,
            last_accessed,
        })
    }

    fn create_directory(path: String) -> Result<(), String> {
        eprintln!("Trying to create directory {path}");
        fs::create_dir_all(path.clone()).map_err(|err| err.to_string())?;
        eprintln!("Finished creating directory {path}");
        Ok(())
    }

    fn create_link(source: String, destination: String) -> Result<(), String> {
        fs::hard_link(source, destination).map_err(|err| err.to_string())
    }

    fn create_sym_link(source: String, destination: String) -> Result<(), String> {
        #[allow(deprecated)]
        fs::soft_link(source, destination).map_err(|err| err.to_string())
    }

    fn remove_directory(path: String) -> Result<(), String> {
        fs::remove_dir(path).map_err(|err| err.to_string())
    }

    fn remove_file(path: String) -> Result<(), String> {
        fs::remove_file(path).map_err(|err| err.to_string())
    }

    fn rename_file(source: String, destination: String) -> Result<(), String> {
        fs::rename(source, destination).map_err(|err| err.to_string())
    }
}
