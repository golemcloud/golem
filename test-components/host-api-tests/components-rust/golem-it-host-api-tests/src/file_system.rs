use golem_rust::{agent_definition, agent_implementation, Schema};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::fs::{create_dir_all, read_dir, read_to_string, remove_file, write, File};
use std::hash::{Hash, Hasher};

#[derive(Clone, Schema, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Clone, Schema, Serialize, Deserialize)]
pub struct DirectoriesResult {
    pub count1: u32,
    pub root_entries: Vec<DirEntry>,
    pub test_entries: Vec<DirEntry>,
    pub count2: u32,
}

#[derive(Clone, Schema, Serialize, Deserialize)]
pub struct FileWriteReadDeleteResult {
    pub read_nonexisting: Option<String>,
    pub read_existing: Option<String>,
    pub read_after_delete: Option<String>,
}

#[derive(Clone, Schema, Serialize, Deserialize)]
pub struct FileTimestamps {
    pub modified_secs: u64,
    pub modified_nanos: u32,
    pub accessed_secs: u64,
    pub accessed_nanos: u32,
}

#[derive(Clone, Schema, Serialize, Deserialize)]
pub struct HashResult {
    pub upper: u64,
    pub lower: u64,
}

#[agent_definition]
pub trait FileSystem {
    fn new(name: String) -> Self;

    fn run_directories(&self) -> DirectoriesResult;
    fn run_file_write_read_delete(&self) -> FileWriteReadDeleteResult;
    fn read_file(&self, path: String) -> Result<String, String>;
    fn write_file(&self, path: String, contents: String) -> Result<(), String>;
    fn delete_file(&self, path: String) -> Result<(), String>;
    fn write_file_direct(&self, name: String, contents: String) -> Result<(), String>;
    fn get_file_info(&self, path: String) -> Result<FileTimestamps, String>;
    fn get_info(&self, path: String) -> Result<FileTimestamps, String>;
    fn create_directory(&self, path: String) -> Result<(), String>;
    fn create_link(&self, source: String, destination: String) -> Result<(), String>;
    fn create_sym_link(&self, source: String, destination: String) -> Result<(), String>;
    fn remove_directory(&self, path: String) -> Result<(), String>;
    fn remove_file(&self, path: String) -> Result<(), String>;
    fn rename_file(&self, source: String, destination: String) -> Result<(), String>;
    fn hash(&self, path: String) -> Result<HashResult, String>;
}

pub struct FileSystemImpl {
    _name: String,
}

#[agent_implementation]
impl FileSystem for FileSystemImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn run_directories(&self) -> DirectoriesResult {
        let rd1 = read_dir("/").unwrap();
        let count1 = rd1.into_iter().count();
        println!("number of entries in root at start: {}", count1);

        let _ = create_dir_all("/test/dir1").unwrap();
        let _ = create_dir_all("/test/dir2").unwrap();
        let _ = write("/test/hello.txt", "hello world").unwrap();

        println!("Created test entries");

        let root_entries: Vec<DirEntry> = read_dir("/")
            .unwrap()
            .into_iter()
            .map(|entry| {
                let dir_entry = entry.unwrap();
                DirEntry {
                    name: dir_entry.path().to_str().unwrap().to_string(),
                    is_dir: dir_entry.metadata().unwrap().is_dir(),
                }
            })
            .collect();

        let test_entries: Vec<DirEntry> = read_dir("/test")
            .unwrap()
            .into_iter()
            .map(|entry| {
                let dir_entry = entry.unwrap();
                DirEntry {
                    name: dir_entry.path().to_str().unwrap().to_string(),
                    is_dir: dir_entry.metadata().unwrap().is_dir(),
                }
            })
            .collect();

        println!("Deleting test entries");

        let rd2 = read_dir("/").unwrap();
        let count2 = rd2.into_iter().count();
        println!("number of entries in root at end: {}", count2);

        DirectoriesResult {
            count1: count1 as u32,
            root_entries,
            test_entries,
            count2: count2 as u32,
        }
    }

    fn run_file_write_read_delete(&self) -> FileWriteReadDeleteResult {
        println!("file write/read/delete test starting");
        let read_nonexisting_result: Option<String> = read_to_string("testfile.txt").ok();
        println!("read_nonexisting_result: {:?}", read_nonexisting_result);
        let _ = write("/testfile.txt", "hello world").unwrap();
        println!("wrote test file");
        let read_existing_result: Option<String> = read_to_string("/testfile.txt").ok();
        println!("read_existing_result: {:?}", read_existing_result);
        let _ = remove_file("/testfile.txt").unwrap();
        println!("deleted test file");
        let read_nonexisting_result2: Option<String> = read_to_string("/testfile.txt").ok();
        println!("read_nonexisting_result2: {:?}", read_nonexisting_result2);

        FileWriteReadDeleteResult {
            read_nonexisting: read_nonexisting_result,
            read_existing: read_existing_result,
            read_after_delete: read_nonexisting_result2,
        }
    }

    fn read_file(&self, path: String) -> Result<String, String> {
        read_to_string(path).map_err(|e| e.to_string())
    }

    fn write_file(&self, path: String, contents: String) -> Result<(), String> {
        write(path, contents).map_err(|e| e.to_string())
    }

    fn delete_file(&self, path: String) -> Result<(), String> {
        remove_file(path).map_err(|e| e.to_string())
    }

    fn write_file_direct(&self, name: String, contents: String) -> Result<(), String> {
        let path = if name.starts_with('/') {
            name
        } else {
            format!("/{name}")
        };
        fs::write(path, contents).map_err(|e| e.to_string())
    }

    fn get_file_info(&self, path: String) -> Result<FileTimestamps, String> {
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
        Ok(FileTimestamps {
            modified_secs: modified.as_secs(),
            modified_nanos: modified.subsec_nanos(),
            accessed_secs: accessed.as_secs(),
            accessed_nanos: accessed.subsec_nanos(),
        })
    }

    fn get_info(&self, path: String) -> Result<FileTimestamps, String> {
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
        Ok(FileTimestamps {
            modified_secs: modified.as_secs(),
            modified_nanos: modified.subsec_nanos(),
            accessed_secs: accessed.as_secs(),
            accessed_nanos: accessed.subsec_nanos(),
        })
    }

    fn create_directory(&self, path: String) -> Result<(), String> {
        eprintln!("Trying to create directory {path}");
        fs::create_dir_all(path.clone()).map_err(|err| err.to_string())?;
        eprintln!("Finished creating directory {path}");
        Ok(())
    }

    fn create_link(&self, source: String, destination: String) -> Result<(), String> {
        fs::hard_link(source, destination).map_err(|err| err.to_string())
    }

    fn create_sym_link(&self, source: String, destination: String) -> Result<(), String> {
        #[allow(deprecated)]
        fs::soft_link(source, destination).map_err(|err| err.to_string())
    }

    fn remove_directory(&self, path: String) -> Result<(), String> {
        fs::remove_dir(path).map_err(|err| err.to_string())
    }

    fn remove_file(&self, path: String) -> Result<(), String> {
        fs::remove_file(path).map_err(|err| err.to_string())
    }

    fn rename_file(&self, source: String, destination: String) -> Result<(), String> {
        fs::rename(source, destination).map_err(|err| err.to_string())
    }

    fn hash(&self, path: String) -> Result<HashResult, String> {
        let full_path = if path.starts_with('/') {
            path
        } else {
            format!("/{path}")
        };
        let metadata = fs::metadata(&full_path).map_err(|err| err.to_string())?;
        let modified = metadata
            .modified()
            .map_err(|err| err.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|err| err.to_string())?;
        let len = metadata.len();

        let mut hasher = DefaultHasher::new();
        len.hash(&mut hasher);
        modified.as_nanos().hash(&mut hasher);
        let upper = hasher.finish();

        let mut hasher2 = DefaultHasher::new();
        full_path.hash(&mut hasher2);
        let lower = hasher2.finish();

        Ok(HashResult { upper, lower })
    }
}
