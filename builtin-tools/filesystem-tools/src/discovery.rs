use crate::{FilesystemToolError, invalid_path, io_error};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use golem_rust::{FromSchema, FromWire, IntoSchema, IntoWire, WireSchema};
use std::path::{Component, Path, PathBuf};

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 500;
const DEFAULT_MAX_DEPTH: u32 = 1;
const MAX_DEPTH: u32 = 32;
pub(super) const MAX_PATH_BYTES: usize = 4 * 1024;
const MAX_GLOBS: usize = 16;
const MAX_GLOB_BYTES: usize = 1024;
const MAX_TOTAL_GLOB_BYTES: usize = 8 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_DIRECTORY_NAME_BYTES: usize = 256 * 1024;
pub(super) const MAX_ENTRIES_EXAMINED: usize = 16_384;
pub(super) const MAX_TRAVERSAL_ENTRIES: usize = 1_024;
pub(super) const MAX_DIAGNOSTICS: usize = 32;
const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 512;
pub(super) const MAX_CURSOR_BYTES: usize = 32 * 1024;
const MAX_OUTPUT_BYTES: usize = 128 * 1024;
pub(super) const MAX_CONTENT_OUTPUT_BYTES: usize = MAX_OUTPUT_BYTES - MAX_CURSOR_BYTES;

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub enum DiagnosticKind {
    Io,
    Binary,
    Unsupported,
    SymlinkSkipped,
    DirectoryTooLarge,
    FileTooLarge,
    LineTooLong,
    PathTooLong,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct DiscoveryDiagnostic {
    pub path: String,
    pub kind: DiagnosticKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct DirectoryFrame {
    /// Last consumed child name in this directory.
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct TreeCursor {
    /// Root-first depth-first traversal stack.
    pub frames: Vec<DirectoryFrame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NodeKind {
    File(u64),
    Directory,
    Symlink,
    Other,
}

/// Entry type as a directory read reports it. A file size needs one more `stat_at`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChildKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug)]
pub(super) enum FsError {
    NotFound,
    Symlink,
    DirectoryTooLarge,
    WorkBudgetExhausted,
    Io(String),
}

#[derive(Debug, Clone)]
struct Child {
    name: String,
    kind: ChildKind,
}

pub(super) struct FsNode {
    descriptor: golem_rust::wasip3::filesystem::types::Descriptor,
    kind: NodeKind,
}

impl FsNode {
    pub(super) async fn open_root(path: &str) -> Result<Self, FsError> {
        use golem_rust::wasip3::filesystem::preopens::get_directories;
        use golem_rust::wasip3::filesystem::types::{
            DescriptorFlags, DescriptorType, OpenFlags, PathFlags,
        };

        let requested = Path::new(path);
        let mut selected = None;
        for (descriptor, guest_path) in get_directories() {
            let guest = Path::new(&guest_path);
            let relative = if requested.is_absolute() {
                requested.strip_prefix(guest).ok().map(Path::to_path_buf)
            } else if guest == Path::new(".") || guest.as_os_str().is_empty() {
                Some(requested.to_path_buf())
            } else {
                None
            };
            if let Some(relative) = relative {
                let specificity = guest.components().count();
                if selected
                    .as_ref()
                    .is_none_or(|(best, _, _): &(usize, _, PathBuf)| specificity > *best)
                {
                    selected = Some((specificity, descriptor, relative));
                }
            }
        }
        let (_, mut descriptor, relative) = selected
            .ok_or_else(|| FsError::Io(format!("no preopened directory contains '{path}'")))?;
        let mut kind = NodeKind::Directory;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                if component == Component::CurDir {
                    continue;
                }
                return Err(FsError::Io("invalid path component".to_string()));
            };
            let name = name
                .to_str()
                .ok_or_else(|| FsError::Io("path is not UTF-8".to_string()))?;
            let stat = descriptor
                .stat_at(PathFlags::empty(), name.to_string())
                .await
                .map_err(map_wasi_error)?;
            if matches!(stat.type_, DescriptorType::SymbolicLink) {
                return Err(FsError::Symlink);
            }
            kind = wasi_kind(stat.type_.clone(), stat.size);
            descriptor = descriptor
                .open_at(
                    PathFlags::empty(),
                    name.to_string(),
                    if matches!(stat.type_, DescriptorType::Directory) {
                        OpenFlags::DIRECTORY
                    } else {
                        OpenFlags::empty()
                    },
                    DescriptorFlags::READ,
                )
                .await
                .map_err(map_wasi_error)?;
        }
        Ok(Self { descriptor, kind })
    }

    pub(super) fn kind(&self) -> NodeKind {
        self.kind
    }

    async fn children(&self, remaining_entries: &mut usize) -> Result<Vec<Child>, FsError> {
        let (mut stream, completion) = self.descriptor.read_directory();
        let mut children = Vec::new();
        let mut name_bytes = 0usize;
        while let Some(entry) = stream.next().await {
            let Some(remaining) = remaining_entries.checked_sub(1) else {
                return Err(FsError::WorkBudgetExhausted);
            };
            *remaining_entries = remaining;
            name_bytes = name_bytes.saturating_add(entry.name.len());
            if children.len() >= MAX_DIRECTORY_ENTRIES || name_bytes > MAX_DIRECTORY_NAME_BYTES {
                return Err(FsError::DirectoryTooLarge);
            }
            children.push(Child {
                kind: child_kind(&entry.type_),
                name: entry.name,
            });
        }
        drop(stream);
        completion.await.map_err(map_wasi_error)?;
        children.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
        Ok(children)
    }

    pub(super) async fn open_child(&self, name: &str) -> Result<Self, FsError> {
        use golem_rust::wasip3::filesystem::types::{
            DescriptorFlags, DescriptorType, OpenFlags, PathFlags,
        };

        let stat = self
            .descriptor
            .stat_at(PathFlags::empty(), name.to_string())
            .await
            .map_err(map_wasi_error)?;
        if matches!(stat.type_, DescriptorType::SymbolicLink) {
            return Err(FsError::Symlink);
        }
        let kind = wasi_kind(stat.type_.clone(), stat.size);
        let descriptor = self
            .descriptor
            .open_at(
                PathFlags::empty(),
                name.to_string(),
                if matches!(stat.type_, DescriptorType::Directory) {
                    OpenFlags::DIRECTORY
                } else {
                    OpenFlags::empty()
                },
                DescriptorFlags::READ,
            )
            .await
            .map_err(map_wasi_error)?;
        Ok(Self { descriptor, kind })
    }

    async fn file_size(&self, name: &str) -> Result<u64, FsError> {
        use golem_rust::wasip3::filesystem::types::PathFlags;

        let stat = self
            .descriptor
            .stat_at(PathFlags::empty(), name.to_string())
            .await
            .map_err(map_wasi_error)?;
        Ok(stat.size)
    }

    pub(super) async fn read_bounded(&self, limit: usize) -> Result<(Vec<u8>, bool), FsError> {
        let (stream, completion) = self.descriptor.read_via_stream(0);
        let (bytes, at_eof) =
            crate::bounded_stream::read_bounded(stream, limit, |mut stream, bytes| async move {
                let (_, returned) = stream.read(bytes).await;
                (stream, returned)
            })
            .await;
        if at_eof {
            completion.await.map_err(map_wasi_error)?;
        }
        Ok((bytes, at_eof))
    }
}

fn map_wasi_error(error: golem_rust::wasip3::filesystem::types::ErrorCode) -> FsError {
    use golem_rust::wasip3::filesystem::types::ErrorCode;

    match error {
        ErrorCode::NoEntry => FsError::NotFound,
        ErrorCode::Loop => FsError::Symlink,
        _ => FsError::Io(format!("{error:?}")),
    }
}

fn wasi_kind(type_: golem_rust::wasip3::filesystem::types::DescriptorType, size: u64) -> NodeKind {
    use golem_rust::wasip3::filesystem::types::DescriptorType;

    match type_ {
        DescriptorType::RegularFile => NodeKind::File(size),
        DescriptorType::Directory => NodeKind::Directory,
        DescriptorType::SymbolicLink => NodeKind::Symlink,
        _ => NodeKind::Other,
    }
}

fn child_kind(type_: &golem_rust::wasip3::filesystem::types::DescriptorType) -> ChildKind {
    use golem_rust::wasip3::filesystem::types::DescriptorType;

    match type_ {
        DescriptorType::RegularFile => ChildKind::File,
        DescriptorType::Directory => ChildKind::Directory,
        DescriptorType::SymbolicLink => ChildKind::Symlink,
        _ => ChildKind::Other,
    }
}

/// One directory on the path of the cursor, opened and read at most once per tool call.
struct Level {
    /// The subdirectory and its name in the parent level; `None` for the root.
    directory: Option<(String, FsNode)>,
    /// Sorted entries, read when this level is first the innermost one.
    children: Option<Vec<Child>>,
}

pub(super) struct TreeWalker<'a> {
    root: &'a FsNode,
    root_path: String,
    max_depth: u32,
    pub(super) cursor: TreeCursor,
    /// `levels[i]` is the directory of `cursor.frames[i]`. Callers may change the cursor, so
    /// each step first drops the levels that the cursor no longer passes through.
    levels: Vec<Level>,
}

impl<'a> TreeWalker<'a> {
    pub(super) fn new(
        root: &'a FsNode,
        root_path: String,
        max_depth: u32,
        cursor: TreeCursor,
    ) -> Self {
        Self {
            root,
            root_path,
            max_depth,
            cursor,
            levels: Vec::new(),
        }
    }

    pub(super) async fn next(
        &mut self,
        descend: impl Fn(&str) -> bool,
        remaining_entries: &mut usize,
    ) -> Result<Option<(String, ChildKind)>, WalkerError> {
        loop {
            if self.cursor.frames.is_empty() {
                return Ok(None);
            }
            self.read_innermost_level(remaining_entries).await?;
            let after = self
                .cursor
                .frames
                .last()
                .and_then(|frame| frame.after.as_deref());
            let children = self
                .levels
                .last()
                .and_then(|level| level.children.as_deref())
                .expect("the innermost level has been read");
            let position = after.map_or(0, |after| {
                children.partition_point(|child| child.name.as_bytes() <= after.as_bytes())
            });
            let Some(child) = children.get(position).cloned() else {
                self.cursor.frames.pop();
                continue;
            };
            self.cursor.frames.last_mut().unwrap().after = Some(child.name.clone());
            let relative = self.current_entry_relative_path();
            if matches!(child.kind, ChildKind::Directory)
                && self.cursor.frames.len() < self.max_depth as usize
                && descend(&relative)
            {
                self.cursor.frames.push(DirectoryFrame { after: None });
            }
            return Ok(Some((relative, child.kind)));
        }
    }

    /// Reads the size of the entry that the cursor selects. Call it after `next` returns a file.
    pub(super) async fn selected_file_size(&mut self) -> Result<u64, WalkerError> {
        let path = join_path(&self.root_path, &self.current_entry_relative_path());
        let (directory, name) = self.selected_entry().await?;
        directory
            .file_size(&name)
            .await
            .map_err(|error| WalkerError {
                path,
                error,
                budget_exhausted: false,
            })
    }

    /// Opens the entry that the cursor selects, from the directory that holds it.
    pub(super) async fn open_selected(&mut self) -> Result<FsNode, WalkerError> {
        let path = join_path(&self.root_path, &self.current_entry_relative_path());
        let (directory, name) = self.selected_entry().await?;
        directory
            .open_child(&name)
            .await
            .map_err(|error| WalkerError {
                path,
                error,
                budget_exhausted: false,
            })
    }

    async fn selected_entry(&mut self) -> Result<(&FsNode, String), WalkerError> {
        self.open_levels().await?;
        let name = self
            .cursor
            .frames
            .last()
            .and_then(|frame| frame.after.clone())
            .ok_or_else(|| WalkerError {
                path: self.current_directory_path(),
                error: FsError::Io("cursor does not select an entry".to_string()),
                budget_exhausted: false,
            })?;
        Ok((self.directory_at(self.levels.len() - 1), name))
    }

    async fn read_innermost_level(
        &mut self,
        remaining_entries: &mut usize,
    ) -> Result<(), WalkerError> {
        self.open_levels().await?;
        let depth = self.levels.len() - 1;
        if self.levels[depth].children.is_none() {
            let children = self
                .directory_at(depth)
                .children(remaining_entries)
                .await
                .map_err(|error| WalkerError {
                    path: self.current_directory_path(),
                    budget_exhausted: matches!(error, FsError::WorkBudgetExhausted),
                    error,
                })?;
            self.levels[depth].children = Some(children);
        }
        Ok(())
    }

    /// Opens each directory of the cursor that has no open level yet.
    async fn open_levels(&mut self) -> Result<(), WalkerError> {
        self.drop_stale_levels();
        while self.levels.len() < self.cursor.frames.len() {
            let depth = self.levels.len();
            let directory = if depth == 0 {
                None
            } else {
                Some(self.open_level(depth).await?)
            };
            self.levels.push(Level {
                directory,
                children: None,
            });
        }
        Ok(())
    }

    async fn open_level(&self, depth: usize) -> Result<(String, FsNode), WalkerError> {
        let name = self.cursor.frames[depth - 1]
            .after
            .clone()
            .ok_or_else(|| WalkerError {
                path: self.root_path.clone(),
                error: FsError::Io("cursor directory frame has no parent child".to_string()),
                budget_exhausted: false,
            })?;
        let directory = self
            .directory_at(depth - 1)
            .open_child(&name)
            .await
            .map_err(|error| WalkerError {
                path: self.directory_path(depth),
                error,
                budget_exhausted: false,
            })?;
        if !matches!(directory.kind(), NodeKind::Directory) {
            return Err(WalkerError {
                path: self.directory_path(depth),
                error: FsError::Io("continued directory is no longer a directory".to_string()),
                budget_exhausted: false,
            });
        }
        Ok((name, directory))
    }

    fn drop_stale_levels(&mut self) {
        let frames = &self.cursor.frames;
        let mut valid = self.levels.len().min(frames.len());
        for depth in 1..valid {
            let name = self.levels[depth]
                .directory
                .as_ref()
                .map(|(name, _)| name.as_str());
            if name != frames[depth - 1].after.as_deref() {
                valid = depth;
                break;
            }
        }
        self.levels.truncate(valid);
    }

    fn directory_at(&self, depth: usize) -> &FsNode {
        self.levels[depth]
            .directory
            .as_ref()
            .map_or(self.root, |(_, directory)| directory)
    }

    fn directory_path(&self, depth: usize) -> String {
        let relative = self.cursor.frames[..depth]
            .iter()
            .filter_map(|frame| frame.after.as_deref())
            .collect::<Vec<_>>()
            .join("/");
        join_path(&self.root_path, &relative)
    }

    fn current_directory_path(&self) -> String {
        self.directory_path(self.cursor.frames.len().saturating_sub(1))
    }

    fn current_entry_relative_path(&self) -> String {
        self.cursor
            .frames
            .iter()
            .filter_map(|frame| frame.after.as_deref())
            .collect::<Vec<_>>()
            .join("/")
    }
}

#[derive(Debug)]
pub(super) struct WalkerError {
    pub(super) path: String,
    pub(super) error: FsError,
    pub(super) budget_exhausted: bool,
}

pub(super) fn validate_discovery_path(path: &str) -> Result<(), FilesystemToolError> {
    if path.is_empty() {
        return Err(invalid_path(path, "path must not be empty"));
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(invalid_path(path, "path exceeds the 4 KiB limit"));
    }
    if path.contains('\0') {
        return Err(invalid_path(path, "path must not contain NUL bytes"));
    }
    if Path::new(path)
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(invalid_path(
            path,
            "parent (`..`) components are not allowed",
        ));
    }
    Ok(())
}

pub(super) async fn open_command_root(path: &str) -> Result<FsNode, FilesystemToolError> {
    FsNode::open_root(path).await.map_err(|error| match error {
        FsError::NotFound => FilesystemToolError::NotFound(path.to_string()),
        FsError::Symlink => FilesystemToolError::SymlinkPath(path.to_string()),
        FsError::DirectoryTooLarge => FilesystemToolError::Io(format!(
            "filesystem operation on '{path}' failed: directory exceeds enumeration limit"
        )),
        FsError::WorkBudgetExhausted => FilesystemToolError::Io(format!(
            "filesystem operation on '{path}' exhausted its work budget"
        )),
        FsError::Io(message) => io_error(path, std::io::Error::other(message)),
    })
}

pub(super) fn effective_depth(depth: Option<u32>) -> Result<u32, FilesystemToolError> {
    let depth = depth.unwrap_or(DEFAULT_MAX_DEPTH);
    if depth == 0 || depth > MAX_DEPTH {
        return Err(FilesystemToolError::InvalidOptions(format!(
            "max_depth must be between 1 and {MAX_DEPTH}"
        )));
    }
    Ok(depth)
}

pub(super) fn effective_limit(limit: Option<u32>) -> Result<u32, FilesystemToolError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(FilesystemToolError::InvalidOptions(format!(
            "limit must be between 1 and {MAX_LIMIT}"
        )));
    }
    Ok(limit)
}

pub(super) fn validate_glob_inputs(
    includes: &[String],
    excludes: &[String],
) -> Result<(), FilesystemToolError> {
    if includes.len() + excludes.len() > MAX_GLOBS {
        return Err(FilesystemToolError::InvalidGlob(format!(
            "at most {MAX_GLOBS} include and exclude globs are allowed"
        )));
    }
    let total = includes
        .iter()
        .chain(excludes)
        .try_fold(0usize, |total, pattern| {
            if pattern.len() > MAX_GLOB_BYTES {
                Err(FilesystemToolError::InvalidGlob(format!(
                    "glob exceeds {MAX_GLOB_BYTES} bytes"
                )))
            } else {
                Ok(total + pattern.len())
            }
        })?;
    if total > MAX_TOTAL_GLOB_BYTES {
        return Err(FilesystemToolError::InvalidGlob(format!(
            "combined glob input exceeds {MAX_TOTAL_GLOB_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(super) fn compile_globs(patterns: &[String]) -> Result<GlobSet, FilesystemToolError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .backslash_escape(true)
            .build()
            .map_err(|error| FilesystemToolError::InvalidGlob(error.to_string()))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| FilesystemToolError::InvalidGlob(error.to_string()))
}

pub(super) fn validate_tree_cursor(
    tree: &TreeCursor,
    max_depth: u32,
) -> Result<(), FilesystemToolError> {
    if tree.frames.is_empty()
        || tree.frames.len() > max_depth as usize
        || estimated_tree_cursor_bytes(tree) > MAX_CURSOR_BYTES
    {
        return Err(FilesystemToolError::InvalidCursor(
            "tree cursor exceeds its depth or size limit".to_string(),
        ));
    }
    for (index, frame) in tree.frames.iter().enumerate() {
        if index + 1 < tree.frames.len() && frame.after.is_none() {
            return Err(FilesystemToolError::InvalidCursor(
                "non-leaf cursor frame has no selected child".to_string(),
            ));
        }
        if let Some(name) = &frame.after
            && (name.is_empty()
                || name == "."
                || name == ".."
                || name.contains('/')
                || name.contains('\0'))
        {
            return Err(FilesystemToolError::InvalidCursor(
                "cursor contains an invalid child name".to_string(),
            ));
        }
    }
    Ok(())
}

pub(super) fn estimated_tree_cursor_bytes(tree: &TreeCursor) -> usize {
    tree.frames
        .iter()
        .map(|frame| frame.after.as_ref().map_or(1, |name| name.len() + 1))
        .sum()
}

pub(super) fn join_path(root: &str, relative: &str) -> String {
    if relative.is_empty() {
        return root.to_string();
    }
    if root == "/" {
        format!("/{relative}")
    } else if root.ends_with('/') {
        format!("{root}{relative}")
    } else {
        format!("{root}/{relative}")
    }
}

pub(super) fn push_walker_diagnostic(
    diagnostics: &mut Vec<DiscoveryDiagnostic>,
    output_bytes: &mut usize,
    error: &WalkerError,
    fallback: DiagnosticKind,
) -> bool {
    let kind = match error.error {
        FsError::DirectoryTooLarge => DiagnosticKind::DirectoryTooLarge,
        FsError::Symlink => DiagnosticKind::SymlinkSkipped,
        _ => fallback,
    };
    push_diagnostic(
        diagnostics,
        output_bytes,
        error.path.clone(),
        kind,
        &fs_error_message(&error.error),
    )
}

pub(super) fn push_diagnostic(
    diagnostics: &mut Vec<DiscoveryDiagnostic>,
    output_bytes: &mut usize,
    path: String,
    kind: DiagnosticKind,
    message: &str,
) -> bool {
    if diagnostics.len() >= MAX_DIAGNOSTICS {
        return false;
    }
    let (message, _) = truncate_utf8(message, MAX_DIAGNOSTIC_MESSAGE_BYTES);
    let size = path.len() + message.len() + 32;
    if output_bytes.saturating_add(size) > MAX_CONTENT_OUTPUT_BYTES {
        return false;
    }
    *output_bytes += size;
    diagnostics.push(DiscoveryDiagnostic {
        path,
        kind,
        message: message.to_string(),
    });
    true
}

pub(super) fn fs_error_message(error: &FsError) -> String {
    match error {
        FsError::NotFound => "filesystem entry was not found".to_string(),
        FsError::Symlink => "symbolic link was not followed".to_string(),
        FsError::DirectoryTooLarge => "directory exceeds enumeration limit".to_string(),
        FsError::WorkBudgetExhausted => "filesystem work budget exhausted".to_string(),
        FsError::Io(message) => message.clone(),
    }
}

pub(super) fn truncate_utf8(value: &str, max_bytes: usize) -> (&str, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], true)
}
