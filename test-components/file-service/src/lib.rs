#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem::it::api::{Datetime, FileInfo, Guest, MetadataHashValue};
use crate::bindings::wasi::filesystem::preopens::get_directories;
use crate::bindings::wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};
use std::fs;

use std::fs::{read_to_string, remove_file, write, File};
use std::path::Path;

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
        let (root, _) = get_directories()
            .into_iter()
            .find(|(_, path)| path == "/")
            .ok_or("Root not found")?;
        let file = root
            .open_at(
                PathFlags::empty(),
                &name,
                OpenFlags::CREATE,
                DescriptorFlags::WRITE,
            )
            .map_err(|_| "Failed to open file")?;
        let _ = file
            .write(contents.as_bytes(), 0)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn get_file_info(path: String) -> Result<FileInfo, String> {
        let file = File::open(path).map_err(|err| err.to_string())?;
        let metadata = file.metadata().map_err(|err| err.to_string())?;
        let modified = metadata
            .modified()
            .map_err(|err| err.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|err| err.to_string())?;
        let accessed = metadata
            .accessed()
            .map_err(|err| err.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|err| err.to_string())?;
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
        let modified = metadata
            .modified()
            .map_err(|err| err.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|err| err.to_string())?;
        let accessed = metadata
            .accessed()
            .map_err(|err| err.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|err| err.to_string())?;
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

    fn hash(path: String) -> Result<MetadataHashValue, String> {
        let (root, _) = get_directories()
            .into_iter()
            .find(|(_, path)| path == "/")
            .ok_or("Root not found")?;
        root.metadata_hash_at(PathFlags::empty(), &path)
            .map_err(|err| err.to_string())
    }

    fn remove_dir_all(path: String) -> Result<(), String> {
        print_tree(Path::new(&path), 0);
        fs::remove_dir_all(path).map_err(|err| err.to_string())
    }

    fn reproducer() {
        let r = Self::create_directory("/tmp/py/modules/0/mytest/__pycache__".to_string());
        println!("{r:?}");

        println!("Creating files");

        let r = Self::write_file(
            "/tmp/py/modules/0/mytest/__init__.py".to_string(),
            "# hello world".to_string(),
        );
        println!("{r:?}");
        let r = Self::write_file(
            "/tmp/py/modules/0/mytest/__pycache__/__init__.rustpython-01.pyc".to_string(),
            "# hello world".to_string(),
        );
        println!("{r:?}");
        let r = Self::write_file(
            "/tmp/py/modules/0/mytest/__pycache__/mymodule.rustpython-01.pyc".to_string(),
            "# hello world".to_string(),
        );
        println!("{r:?}");
        let r = Self::write_file(
            "/tmp/py/modules/0/mytest/mymodule.py".to_string(),
            "# hello world".to_string(),
        );
        println!("{r:?}");

        println!("Removing all");

        let r = Self::remove_dir_all("/tmp/py/modules/0".to_string());
        println!("{r:?}");
    }
}

fn print_tree(path: &Path, indent: usize) {
    println!("print_tree {path:?}");
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            for _ in 0..indent {
                print!("  ");
            }
            if path.is_dir() {
                println!("📁 {}", path.file_name().unwrap().to_string_lossy());
                print_tree(&path, indent + 1);
            } else {
                println!("📄 {}", path.file_name().unwrap().to_string_lossy());
            }
        }
    }
}

bindings::export!(Component with_types_in bindings);
