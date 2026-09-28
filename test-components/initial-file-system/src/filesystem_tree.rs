use golem_rust::{agent_definition, agent_implementation};
use std::fs;
use std::path::{Path, PathBuf};
use wasi::filesystem::preopens;
use wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};

#[agent_definition]
pub trait FilesystemTree {
    fn new(name: String) -> Self;
    /// Applies one filesystem operation (`remove`, `write`, `rename` or `link`) and gives `ok`
    /// or the error.
    fn apply(&self, operation: String, path: String, argument: String) -> String;
    /// Lists every path below the root in path order: each directory, and each file with its
    /// content, its link count and whether it opens for writing.
    fn describe(&self) -> Vec<String>;
}

struct FilesystemTreeImpl {
    _name: String,
}

fn outcome(result: std::io::Result<()>) -> String {
    result.map_or_else(
        |error| format!("err:{:?}", error.kind()),
        |()| "ok".to_string(),
    )
}

/// Applies one filesystem operation and gives `ok` or the error.
fn apply_operation(operation: &str, path: &str, argument: &str) -> String {
    match operation {
        "remove" => outcome(fs::remove_file(path)),
        "write" => outcome(fs::write(path, argument)),
        "rename" => outcome(fs::rename(path, argument)),
        "link" => outcome(fs::hard_link(path, argument)),
        "mkdir" => outcome(fs::create_dir(path)),
        "rmdir" => outcome(fs::remove_dir(path)),
        "symlink" => {
            let (root, _) = preopens::get_directories()
                .into_iter()
                .next()
                .expect("no preopened directory");
            root.symlink_at(argument, path)
                .map_or_else(|error| format!("err:{error:?}"), |()| "ok".to_string())
        }
        other => format!("err:unknown operation {other}"),
    }
}

/// Lists every path below the root in path order: each directory, each symlink with its target,
/// and each file with its content, its link count and whether it opens for writing.
fn describe_tree() -> Vec<String> {
    let (root, _) = preopens::get_directories()
        .into_iter()
        .next()
        .expect("no preopened directory");
    list(PathBuf::new(), Vec::new())
        .into_iter()
        .map(|path| {
            let name = path.to_string_lossy().into_owned();
            let metadata = fs::symlink_metadata(&path).expect("read metadata");
            if metadata.is_dir() {
                format!("{name} dir")
            } else if metadata.file_type().is_symlink() {
                let target = fs::read_link(&path).expect("read symlink");
                format!("{name} symlink target={}", target.to_string_lossy())
            } else {
                let links = root
                    .stat_at(PathFlags::empty(), &name)
                    .expect("stat file")
                    .link_count;
                let writable = root
                    .open_at(
                        PathFlags::empty(),
                        &name,
                        OpenFlags::empty(),
                        DescriptorFlags::WRITE,
                    )
                    .is_ok();
                let content = fs::read_to_string(&path).expect("read file");
                format!("{name} file links={links} writable={writable} content={content:?}")
            }
        })
        .collect()
}

fn list(relative: PathBuf, found: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(Path::new(".").join(&relative))
        .expect("read directory")
        .map(|entry| relative.join(entry.expect("read directory entry").file_name()))
        .collect::<Vec<_>>();
    entries.sort();
    entries.into_iter().fold(found, |mut found, path| {
        let directory = fs::symlink_metadata(&path).expect("read metadata").is_dir();
        found.push(path.clone());
        if directory { list(path, found) } else { found }
    })
}

#[agent_implementation]
impl FilesystemTree for FilesystemTreeImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn apply(&self, operation: String, path: String, argument: String) -> String {
        apply_operation(&operation, &path, &argument)
    }

    fn describe(&self) -> Vec<String> {
        describe_tree()
    }
}

/// A filesystem agent with snapshots. It counts the operations that it applied, and its
/// application snapshot holds the count, so a start from a snapshot shows whether the replay
/// after the snapshot ran.
#[agent_definition(snapshotting = "enabled")]
pub trait SnapshotTree {
    fn new(name: String) -> Self;
    /// Applies one filesystem operation (`remove`, `write`, `rename`, `link`, `mkdir`, `rmdir` or
    /// `symlink`) and gives `ok` or the error.
    fn apply(&mut self, operation: String, path: String, argument: String) -> String;
    /// Lists every path below the root in path order.
    fn describe(&self) -> Vec<String>;
    /// Gives the number of operations that the agent applied.
    fn applied(&self) -> u32;
}

struct SnapshotTreeImpl {
    _name: String,
    applied: u32,
}

#[agent_implementation]
impl SnapshotTree for SnapshotTreeImpl {
    fn new(name: String) -> Self {
        // The guest reads its environment once, at its first access. The constructor makes that
        // access, so the first snapshot comes after it, and a start from that snapshot replays
        // the same host calls that the live run made.
        let _ = std::env::vars().count();
        Self {
            _name: name,
            applied: 0,
        }
    }

    fn apply(&mut self, operation: String, path: String, argument: String) -> String {
        self.applied += 1;
        apply_operation(&operation, &path, &argument)
    }

    fn describe(&self) -> Vec<String> {
        describe_tree()
    }

    fn applied(&self) -> u32 {
        self.applied
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.applied.to_le_bytes().to_vec())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let applied = <[u8; 4]>::try_from(bytes.as_slice())
            .map(u32::from_le_bytes)
            .map_err(|_| format!("Invalid snapshot size: {}", bytes.len()))?;
        let golem_rust::SchemaValue::Record { fields } = context.parameters else {
            return Err("Invalid snapshot restore parameters".to_string());
        };
        let [golem_rust::SchemaValue::String(name)] = fields.as_slice() else {
            return Err("Invalid snapshot restore parameters".to_string());
        };
        Ok(Self {
            _name: name.clone(),
            applied,
        })
    }
}
