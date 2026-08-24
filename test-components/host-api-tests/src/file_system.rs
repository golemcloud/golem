use golem_rust::wasip3::filesystem::preopens as p3_preopens;
use golem_rust::wasip3::filesystem::types as p3_types;
use golem_rust::{
    FromSchema, IntoSchema, PromiseId, agent_definition, agent_implementation,
    blocking_await_promise, create_promise,
};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::fs::{File, create_dir_all, read_dir, read_to_string, remove_file, write};
use std::hash::{Hash, Hasher};
use wasi::filesystem::types::{Descriptor, DescriptorFlags, OpenFlags, PathFlags};
use wasi::io::streams::OutputStream;

#[derive(Clone, IntoSchema, FromSchema, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Clone, IntoSchema, FromSchema, Serialize, Deserialize)]
pub struct DirectoriesResult {
    pub count1: u32,
    pub root_entries: Vec<DirEntry>,
    pub test_entries: Vec<DirEntry>,
    pub count2: u32,
}

#[derive(Clone, IntoSchema, FromSchema, Serialize, Deserialize)]
pub struct FileWriteReadDeleteResult {
    pub read_nonexisting: Option<String>,
    pub read_existing: Option<String>,
    pub read_after_delete: Option<String>,
}

#[derive(Clone, IntoSchema, FromSchema, Serialize, Deserialize)]
pub struct FileTimestamps {
    pub modified_secs: u64,
    pub modified_nanos: u32,
    pub accessed_secs: u64,
    pub accessed_nanos: u32,
}

#[derive(Clone, IntoSchema, FromSchema, Serialize, Deserialize)]
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
    fn create_release_promise(&self) -> PromiseId;
    fn write_after_promise(
        &self,
        path: String,
        contents: String,
        release: PromiseId,
    ) -> Result<(), String>;
    fn hash(&self, path: String) -> Result<HashResult, String>;
    fn stream_to_file(&self, path: String, len: u64) -> Result<(), String>;
    fn stream_to_stdout(&self, len: u64) -> Result<(), String>;
    fn blocking_stream_and_flush_to_file(&self, path: String, len: u64) -> Result<(), String>;
    /// Set the size of a file using `descriptor::set_size` (the WASI pwrite-truncate path).
    /// If `new_size` is larger than the current file size the file is grown (zero-filled).
    /// If smaller, the file is truncated.  This exercises the quota grow/shrink paths.
    fn set_file_size(&self, path: String, new_size: u64) -> Result<(), String>;
    /// Write `contents` at `offset` using `descriptor::write` (the direct pwrite path),
    /// bypassing the output-stream API.  This exercises the descriptor-write quota path.
    fn pwrite_file(&self, path: String, offset: u64, contents: String) -> Result<(), String>;
    /// Read from `src_path` and splice into a new file at `dst_path` using
    /// `output-stream.blocking-splice`.
    fn blocking_splice(&self, src_path: String, dst_path: String) -> Result<u64, String>;
    /// Read from `src_path` and splice into a new file at `dst_path` using
    /// the non-blocking `output-stream.splice`.
    fn splice(&self, src_path: String, dst_path: String) -> Result<u64, String>;
    async fn probe_p2(&self, operation: String, path: String, other: String) -> Result<(), String>;
    async fn probe_p3(&self, operation: String, path: String, other: String) -> Result<(), String>;
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

        create_dir_all("/test/dir1").unwrap();
        create_dir_all("/test/dir2").unwrap();
        write("/test/hello.txt", "hello world").unwrap();

        println!("Created test entries");

        let root_entries: Vec<DirEntry> = read_dir("/")
            .unwrap()
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
        write("/testfile.txt", "hello world").unwrap();
        println!("wrote test file");
        let read_existing_result: Option<String> = read_to_string("/testfile.txt").ok();
        println!("read_existing_result: {:?}", read_existing_result);
        remove_file("/testfile.txt").unwrap();
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

    fn create_release_promise(&self) -> PromiseId {
        create_promise()
    }

    fn write_after_promise(
        &self,
        path: String,
        contents: String,
        release: PromiseId,
    ) -> Result<(), String> {
        let dirs = wasi::filesystem::preopens::get_directories();
        let (root, _) = dirs.into_iter().next().ok_or("no preopened directory")?;
        let fd = root
            .open_at(
                PathFlags::empty(),
                path.trim_start_matches('/'),
                OpenFlags::CREATE,
                DescriptorFlags::WRITE,
            )
            .map_err(|e| format!("{e:?}"))?;
        let stream = fd.write_via_stream(0).map_err(|e| format!("{e:?}"))?;

        blocking_await_promise(&release);
        stream
            .blocking_write_and_flush(contents.as_bytes())
            .map_err(|e| format!("{e:?}"))
    }

    fn stream_to_file(&self, path: String, len: u64) -> Result<(), String> {
        let dirs = wasi::filesystem::preopens::get_directories();
        let (root, _) = dirs.into_iter().next().ok_or("no preopened directory")?;
        let rel = path.trim_start_matches('/');
        let fd: Descriptor = root
            .open_at(
                PathFlags::empty(),
                rel,
                OpenFlags::CREATE,
                DescriptorFlags::WRITE,
            )
            .map_err(|e| format!("{e:?}"))?;
        let stream: OutputStream = fd.write_via_stream(0).map_err(|e| format!("{e:?}"))?;
        stream
            .blocking_write_zeroes_and_flush(len)
            .map_err(|e| format!("{e:?}"))
    }

    fn stream_to_stdout(&self, len: u64) -> Result<(), String> {
        let stdout: OutputStream = wasi::cli::stdout::get_stdout();
        stdout.write_zeroes(len).map_err(|e| format!("{e:?}"))
    }

    fn blocking_stream_and_flush_to_file(&self, path: String, len: u64) -> Result<(), String> {
        let dirs = wasi::filesystem::preopens::get_directories();
        let (root, _) = dirs.into_iter().next().ok_or("no preopened directory")?;
        let rel = path.trim_start_matches('/');
        let fd: Descriptor = root
            .open_at(
                PathFlags::empty(),
                rel,
                OpenFlags::CREATE,
                DescriptorFlags::WRITE,
            )
            .map_err(|e| format!("{e:?}"))?;
        let stream: OutputStream = fd.write_via_stream(0).map_err(|e| format!("{e:?}"))?;
        stream
            .blocking_write_zeroes_and_flush(len)
            .map_err(|e| format!("{e:?}"))
    }

    fn set_file_size(&self, path: String, new_size: u64) -> Result<(), String> {
        let dirs = wasi::filesystem::preopens::get_directories();
        let (root, _) = dirs.into_iter().next().ok_or("no preopened directory")?;
        let rel = path.trim_start_matches('/');
        let fd: Descriptor = root
            .open_at(
                PathFlags::empty(),
                rel,
                OpenFlags::CREATE,
                DescriptorFlags::WRITE,
            )
            .map_err(|e| format!("{e:?}"))?;
        fd.set_size(new_size).map_err(|e| format!("{e:?}"))
    }

    fn pwrite_file(&self, path: String, offset: u64, contents: String) -> Result<(), String> {
        let dirs = wasi::filesystem::preopens::get_directories();
        let (root, _) = dirs.into_iter().next().ok_or("no preopened directory")?;
        let rel = path.trim_start_matches('/');
        let fd: Descriptor = root
            .open_at(
                PathFlags::empty(),
                rel,
                OpenFlags::CREATE,
                DescriptorFlags::WRITE,
            )
            .map_err(|e| format!("{e:?}"))?;
        fd.write(contents.as_bytes(), offset)
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
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

    fn blocking_splice(&self, src_path: String, dst_path: String) -> Result<u64, String> {
        let dirs = wasi::filesystem::preopens::get_directories();
        let (root, _) = dirs.into_iter().next().ok_or("no preopened directory")?;

        let src_rel = src_path.trim_start_matches('/');
        let src_fd: Descriptor = root
            .open_at(
                PathFlags::empty(),
                src_rel,
                OpenFlags::empty(),
                DescriptorFlags::READ,
            )
            .map_err(|e| format!("open src: {e:?}"))?;
        let src_size = src_fd.stat().map_err(|e| format!("stat src: {e:?}"))?.size;
        let input = src_fd
            .read_via_stream(0)
            .map_err(|e| format!("read_via_stream: {e:?}"))?;

        let dst_rel = dst_path.trim_start_matches('/');
        let dst_fd: Descriptor = root
            .open_at(
                PathFlags::empty(),
                dst_rel,
                OpenFlags::CREATE,
                DescriptorFlags::WRITE,
            )
            .map_err(|e| format!("open dst: {e:?}"))?;
        let output: OutputStream = dst_fd
            .write_via_stream(0)
            .map_err(|e| format!("write_via_stream: {e:?}"))?;

        let spliced = output
            .blocking_splice(&input, src_size)
            .map_err(|e| format!("blocking_splice: {e:?}"))?;

        Ok(spliced)
    }

    fn splice(&self, src_path: String, dst_path: String) -> Result<u64, String> {
        let dirs = wasi::filesystem::preopens::get_directories();
        let (root, _) = dirs.into_iter().next().ok_or("no preopened directory")?;

        let src_rel = src_path.trim_start_matches('/');
        let src_fd: Descriptor = root
            .open_at(
                PathFlags::empty(),
                src_rel,
                OpenFlags::empty(),
                DescriptorFlags::READ,
            )
            .map_err(|e| format!("open src: {e:?}"))?;
        let src_size = src_fd.stat().map_err(|e| format!("stat src: {e:?}"))?.size;
        let input = src_fd
            .read_via_stream(0)
            .map_err(|e| format!("read_via_stream: {e:?}"))?;

        let dst_rel = dst_path.trim_start_matches('/');
        let dst_fd: Descriptor = root
            .open_at(
                PathFlags::empty(),
                dst_rel,
                OpenFlags::CREATE,
                DescriptorFlags::WRITE,
            )
            .map_err(|e| format!("open dst: {e:?}"))?;
        let output: OutputStream = dst_fd
            .write_via_stream(0)
            .map_err(|e| format!("write_via_stream: {e:?}"))?;

        // Use non-blocking splice in a loop until all bytes transferred.
        let mut total = 0u64;
        while total < src_size {
            let spliced = output
                .splice(&input, src_size - total)
                .map_err(|e| format!("splice: {e:?}"))?;
            if spliced == 0 {
                break;
            }
            total += spliced;
        }

        Ok(total)
    }

    async fn probe_p2(&self, operation: String, path: String, other: String) -> Result<(), String> {
        let (root, _) = wasi::filesystem::preopens::get_directories()
            .into_iter()
            .next()
            .ok_or("no P2 preopened directory")?;
        let path = path.trim_start_matches('/');
        let other = other.trim_start_matches('/');

        if let Some((open_flags, descriptor_flags)) = match operation.as_str() {
            "open-read" => Some((OpenFlags::empty(), DescriptorFlags::READ)),
            "open-list" => Some((OpenFlags::DIRECTORY, DescriptorFlags::READ)),
            "open-create" => Some((OpenFlags::CREATE, DescriptorFlags::WRITE)),
            "open-write" => Some((OpenFlags::empty(), DescriptorFlags::WRITE)),
            "open-truncate" => Some((OpenFlags::TRUNCATE, DescriptorFlags::WRITE)),
            _ => None,
        } {
            return root
                .open_at(PathFlags::empty(), path, open_flags, descriptor_flags)
                .map(|_| ())
                .map_err(|error| format!("{error:?}"));
        }

        let descriptor = if path.is_empty() {
            root
        } else {
            root.open_at(
                PathFlags::empty(),
                path,
                OpenFlags::empty(),
                DescriptorFlags::READ,
            )
            .map_err(|error| format!("open probe descriptor: {error:?}"))?
        };
        let (destination_root, _) = wasi::filesystem::preopens::get_directories()
            .into_iter()
            .next()
            .ok_or("no P2 destination preopened directory")?;
        let result = match operation.as_str() {
            "read" => descriptor.read(1, 0).map(|_| ()),
            "read-via-stream" => descriptor.read_via_stream(0).map(|_| ()),
            "write" => descriptor.write(&[0], 0).map(|_| ()),
            "write-via-stream" => descriptor.write_via_stream(0).map(|_| ()),
            "append-via-stream" => descriptor.append_via_stream().map(|_| ()),
            "set-size" => descriptor.set_size(0),
            "set-times" => descriptor.set_times(
                wasi::filesystem::types::NewTimestamp::Now,
                wasi::filesystem::types::NewTimestamp::Now,
            ),
            "sync-data" => descriptor.sync_data(),
            "sync" => descriptor.sync(),
            "read-directory" => descriptor.read_directory().map(|_| ()),
            "stat" => descriptor.stat().map(|_| ()),
            "stat-at" => descriptor.stat_at(PathFlags::empty(), other).map(|_| ()),
            "metadata-hash" => descriptor.metadata_hash().map(|_| ()),
            "metadata-hash-at" => descriptor
                .metadata_hash_at(PathFlags::empty(), other)
                .map(|_| ()),
            "readlink-at" => descriptor.readlink_at(other).map(|_| ()),
            "create-directory-at" => descriptor.create_directory_at(other),
            "set-times-at" => descriptor.set_times_at(
                PathFlags::empty(),
                other,
                wasi::filesystem::types::NewTimestamp::Now,
                wasi::filesystem::types::NewTimestamp::Now,
            ),
            "symlink-at" => descriptor.symlink_at(other, "probe-link"),
            "remove-directory-at" => descriptor.remove_directory_at(other),
            "unlink-file-at" => descriptor.unlink_file_at(other),
            "rename-at" => descriptor.rename_at(other, &destination_root, "probe-renamed"),
            "link-at" => {
                descriptor.link_at(PathFlags::empty(), other, &destination_root, "probe-linked")
            }
            _ => {
                return Err(format!(
                    "unknown P2 filesystem probe operation: {operation}"
                ));
            }
        };
        result.map_err(|error| format!("{error:?}"))
    }

    async fn probe_p3(&self, operation: String, path: String, other: String) -> Result<(), String> {
        let (root, _) = p3_preopens::get_directories()
            .into_iter()
            .next()
            .ok_or("no P3 preopened directory")?;
        let path = path.trim_start_matches('/').to_string();
        let other = other.trim_start_matches('/').to_string();

        if let Some((open_flags, descriptor_flags)) = match operation.as_str() {
            "open-read" => Some((
                p3_types::OpenFlags::empty(),
                p3_types::DescriptorFlags::READ,
            )),
            "open-list" => Some((
                p3_types::OpenFlags::DIRECTORY,
                p3_types::DescriptorFlags::READ,
            )),
            "open-create" => Some((
                p3_types::OpenFlags::CREATE,
                p3_types::DescriptorFlags::WRITE,
            )),
            "open-write" => Some((
                p3_types::OpenFlags::empty(),
                p3_types::DescriptorFlags::WRITE,
            )),
            "open-truncate" => Some((
                p3_types::OpenFlags::TRUNCATE,
                p3_types::DescriptorFlags::WRITE,
            )),
            _ => None,
        } {
            return root
                .open_at(
                    p3_types::PathFlags::empty(),
                    path,
                    open_flags,
                    descriptor_flags,
                )
                .await
                .map(|_| ())
                .map_err(|error| format!("{error:?}"));
        }

        let descriptor = if path.is_empty() {
            root
        } else {
            root.open_at(
                p3_types::PathFlags::empty(),
                path,
                p3_types::OpenFlags::empty(),
                p3_types::DescriptorFlags::READ,
            )
            .await
            .map_err(|error| format!("open probe descriptor: {error:?}"))?
        };
        let (destination_root, _) = p3_preopens::get_directories()
            .into_iter()
            .next()
            .ok_or("no P3 destination preopened directory")?;
        let result = match operation.as_str() {
            "read-via-stream" => {
                let (_, completion) = descriptor.read_via_stream(0);
                completion.await
            }
            "write-via-stream" => {
                let (writer, reader) = golem_rust::wasip3::wit_stream::new::<u8>();
                drop(writer);
                descriptor.write_via_stream(reader, 0).await
            }
            "append-via-stream" => {
                let (writer, reader) = golem_rust::wasip3::wit_stream::new::<u8>();
                drop(writer);
                descriptor.append_via_stream(reader).await
            }
            "set-size" => descriptor.set_size(0).await,
            "set-times" => {
                descriptor
                    .set_times(p3_types::NewTimestamp::Now, p3_types::NewTimestamp::Now)
                    .await
            }
            "sync-data" => descriptor.sync_data().await,
            "sync" => descriptor.sync().await,
            "read-directory" => {
                let (_, completion) = descriptor.read_directory();
                completion.await
            }
            "stat" => descriptor.stat().await.map(|_| ()),
            "stat-at" => descriptor
                .stat_at(p3_types::PathFlags::empty(), other)
                .await
                .map(|_| ()),
            "metadata-hash" => descriptor.metadata_hash().await.map(|_| ()),
            "metadata-hash-at" => descriptor
                .metadata_hash_at(p3_types::PathFlags::empty(), other)
                .await
                .map(|_| ()),
            "readlink-at" => descriptor.readlink_at(other).await.map(|_| ()),
            "create-directory-at" => descriptor.create_directory_at(other).await,
            "set-times-at" => {
                descriptor
                    .set_times_at(
                        p3_types::PathFlags::empty(),
                        other,
                        p3_types::NewTimestamp::Now,
                        p3_types::NewTimestamp::Now,
                    )
                    .await
            }
            "symlink-at" => descriptor.symlink_at(other, "probe-link".to_string()).await,
            "remove-directory-at" => descriptor.remove_directory_at(other).await,
            "unlink-file-at" => descriptor.unlink_file_at(other).await,
            "rename-at" => {
                descriptor
                    .rename_at(other, &destination_root, "probe-renamed".to_string())
                    .await
            }
            "link-at" => {
                descriptor
                    .link_at(
                        p3_types::PathFlags::empty(),
                        other,
                        &destination_root,
                        "probe-linked".to_string(),
                    )
                    .await
            }
            _ => {
                return Err(format!(
                    "unknown P3 filesystem probe operation: {operation}"
                ));
            }
        };
        result.map_err(|error| format!("{error:?}"))
    }
}
