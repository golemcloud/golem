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
        match operation.as_str() {
            "remove" => outcome(fs::remove_file(&path)),
            "write" => outcome(fs::write(&path, argument)),
            "rename" => outcome(fs::rename(&path, &argument)),
            "link" => outcome(fs::hard_link(&path, &argument)),
            other => format!("err:unknown operation {other}"),
        }
    }

    fn describe(&self) -> Vec<String> {
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
}
