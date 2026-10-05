use crate::{FilesystemToolError, discovery::*};
use globset::GlobSet;
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, WireSchema, tool_definition, tool_implementation,
};
use regex::{Regex, RegexBuilder};
use sha2::{Digest, Sha256};
use std::path::Path;

const MAX_PATTERN_BYTES: usize = 4 * 1024;
const MAX_FILES_PER_CALL: usize = 64;
const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_BYTES_PER_CALL: usize = 4 * 1024 * 1024;
const MAX_LINES_PER_CALL: usize = 4_096;
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_MATCH_TEXT_BYTES: usize = 4 * 1024;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema,
)]
pub enum SearchMode {
    Literal,
    Regex,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct GrepQuery {
    pub path: String,
    pub pattern: String,
    pub mode: SearchMode,
    pub case_sensitive: bool,
    pub max_depth: u32,
    pub include_globs: Vec<String>,
    pub exclude_globs: Vec<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct GrepFileCursor {
    /// Parent traversal checkpoint, or none for a directly selected file.
    pub tree: Option<TreeCursor>,
    /// Byte offset of the next complete logical line.
    pub next_byte: u64,
    /// One-based number of the next logical line.
    pub next_line: u64,
    /// SHA-256 of the active file, preventing continuation after content changes.
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GrepPosition {
    Tree(TreeCursor),
    File(GrepFileCursor),
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct GrepCursor {
    /// Effective query arguments. They must match the continuation call.
    pub query: GrepQuery,
    /// Root-relative tree checkpoint. Exactly one of `tree` and `file` is present.
    pub tree: Option<TreeCursor>,
    /// Active-file checkpoint. Exactly one of `tree` and `file` is present.
    pub file: Option<GrepFileCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct GrepMatch {
    pub path: String,
    /// One-based line number.
    pub line: u64,
    /// Matching line without its LF or CRLF terminator.
    pub text: String,
    /// Whether the returned text was shortened to the output limit.
    pub text_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct GrepResult {
    pub matches: Vec<GrepMatch>,
    pub diagnostics: Vec<DiscoveryDiagnostic>,
    pub next_cursor: Option<GrepCursor>,
}

#[tool_definition(version = "0.1.0", requires_filesystem = true)]
pub trait Grep {
    /// Searches one UTF-8 file or a directory tree by complete logical lines without following
    /// symbolic links. Literal, case-sensitive matching is the default. Filters use globset
    /// syntax against slash-separated paths relative to the root; exclusions win. Results,
    /// diagnostics, file sizes, traversal work, and returned text are bounded and resumable.
    #[command(annotations(
        read_only = true,
        destructive = false,
        idempotent = true,
        open_world = false
    ))]
    #[allow(clippy::too_many_arguments)]
    async fn grep(
        &self,
        path: String,
        pattern: String,
        mode: Option<SearchMode>,
        case_insensitive: bool,
        max_depth: Option<u32>,
        include_globs: Vec<String>,
        exclude_globs: Vec<String>,
        limit: Option<u32>,
        cursor: Option<GrepCursor>,
    ) -> Result<GrepResult, FilesystemToolError>;
}

struct GrepImpl;

#[tool_implementation]
impl Grep for GrepImpl {
    async fn grep(
        &self,
        path: String,
        pattern: String,
        mode: Option<SearchMode>,
        case_insensitive: bool,
        max_depth: Option<u32>,
        include_globs: Vec<String>,
        exclude_globs: Vec<String>,
        limit: Option<u32>,
        cursor: Option<GrepCursor>,
    ) -> Result<GrepResult, FilesystemToolError> {
        grep(
            path,
            pattern,
            mode,
            case_insensitive,
            max_depth,
            include_globs,
            exclude_globs,
            limit,
            cursor,
        )
        .await
    }
}

pub async fn grep(
    path: String,
    pattern: String,
    mode: Option<SearchMode>,
    case_insensitive: bool,
    max_depth: Option<u32>,
    include_globs: Vec<String>,
    exclude_globs: Vec<String>,
    limit: Option<u32>,
    cursor: Option<GrepCursor>,
) -> Result<GrepResult, FilesystemToolError> {
    validate_discovery_path(&path)?;
    let mode = mode.unwrap_or(SearchMode::Literal);
    validate_pattern(&pattern, mode)?;
    validate_glob_inputs(&include_globs, &exclude_globs)?;
    let query = GrepQuery {
        path: path.clone(),
        pattern: pattern.clone(),
        mode,
        case_sensitive: !case_insensitive,
        max_depth: effective_depth(max_depth)?,
        include_globs,
        exclude_globs,
        limit: effective_limit(limit)?,
    };
    let regex = compile_regex(&query)?;
    let includes = compile_globs(&query.include_globs)?;
    let excludes = compile_globs(&query.exclude_globs)?;
    let root = open_command_root(&path).await?;
    let position = match cursor {
        Some(cursor) => {
            validate_grep_cursor(&cursor, &query)?;
            match (cursor.tree, cursor.file) {
                (Some(tree), None) => GrepPosition::Tree(tree),
                (None, Some(file)) => GrepPosition::File(file),
                _ => unreachable!("validated grep cursor has exactly one position"),
            }
        }
        None => match root.kind() {
            NodeKind::Directory => GrepPosition::Tree(TreeCursor {
                frames: vec![DirectoryFrame { after: None }],
            }),
            NodeKind::File(_) => GrepPosition::File(GrepFileCursor {
                tree: None,
                next_byte: 0,
                next_line: 1,
                content_sha256: String::new(),
            }),
            NodeKind::Symlink => return Err(FilesystemToolError::SymlinkPath(path)),
            NodeKind::Other => return Err(FilesystemToolError::UnsupportedRoot(path)),
        },
    };
    match (&position, root.kind()) {
        (GrepPosition::Tree(_), NodeKind::Directory)
        | (GrepPosition::File(GrepFileCursor { tree: Some(_), .. }), NodeKind::Directory)
        | (GrepPosition::File(GrepFileCursor { tree: None, .. }), NodeKind::File(_)) => {}
        _ => {
            return Err(FilesystemToolError::InvalidCursor(
                "grep cursor position does not match the current root type".to_string(),
            ));
        }
    }

    let mut state = GrepState {
        query,
        regex,
        includes,
        excludes,
        matches: Vec::new(),
        diagnostics: Vec::new(),
        output_bytes: 0,
        bytes_read: 0,
        files_read: 0,
        lines_read: 0,
        traversal_entries: 0,
        remaining_entries: MAX_ENTRIES_EXAMINED,
    };
    let position = state.run(position).await?;
    Ok(GrepResult {
        matches: state.matches,
        diagnostics: state.diagnostics,
        next_cursor: position.map(|position| match position {
            GrepPosition::Tree(tree) => GrepCursor {
                query: state.query,
                tree: Some(tree),
                file: None,
            },
            GrepPosition::File(file) => GrepCursor {
                query: state.query,
                tree: None,
                file: Some(file),
            },
        }),
    })
}

struct GrepState {
    query: GrepQuery,
    regex: Regex,
    includes: GlobSet,
    excludes: GlobSet,
    matches: Vec<GrepMatch>,
    diagnostics: Vec<DiscoveryDiagnostic>,
    output_bytes: usize,
    bytes_read: usize,
    files_read: usize,
    lines_read: usize,
    traversal_entries: usize,
    remaining_entries: usize,
}

impl GrepState {
    async fn run(
        &mut self,
        mut position: GrepPosition,
    ) -> Result<Option<GrepPosition>, FilesystemToolError> {
        loop {
            if self.matches.len() >= self.query.limit as usize
                || self.files_read >= MAX_FILES_PER_CALL
                || self.bytes_read >= MAX_BYTES_PER_CALL
                || self.lines_read >= MAX_LINES_PER_CALL
                || self.traversal_entries >= MAX_TRAVERSAL_ENTRIES
                || self.diagnostics.len() >= MAX_DIAGNOSTICS
            {
                return Ok(Some(position));
            }
            position = match position {
                GrepPosition::File(file) => match self.search_file(file).await? {
                    FileProgress::Continue(file) => return Ok(Some(GrepPosition::File(file))),
                    FileProgress::Done(Some(tree)) => GrepPosition::Tree(tree),
                    FileProgress::Done(None) => return Ok(None),
                },
                GrepPosition::Tree(tree) => {
                    let tree = self.prune_excluded_ancestors(tree);
                    let mut walker =
                        TreeWalker::new(self.query.path.clone(), self.query.max_depth, tree);
                    let checkpoint = walker.cursor.clone();
                    match walker
                        .next(
                            |relative| !self.excludes.is_match(relative),
                            &mut self.remaining_entries,
                        )
                        .await
                    {
                        Ok(Some((relative, kind))) => {
                            self.traversal_entries += 1;
                            let full_path = join_path(&self.query.path, &relative);
                            let filtered_non_directory = !matches!(kind, NodeKind::Directory)
                                && (self.excludes.is_match(&relative)
                                    || (!self.query.include_globs.is_empty()
                                        && !self.includes.is_match(&relative)));
                            if filtered_non_directory {
                                GrepPosition::Tree(walker.cursor)
                            } else if full_path.len() > MAX_PATH_BYTES {
                                if !self.diagnostic(
                                    full_path,
                                    DiagnosticKind::PathTooLong,
                                    "entry path exceeds the 4 KiB limit",
                                ) {
                                    return Ok(Some(GrepPosition::Tree(checkpoint)));
                                }
                                GrepPosition::Tree(walker.cursor)
                            } else {
                                match kind {
                                    NodeKind::File(_) => GrepPosition::File(GrepFileCursor {
                                        tree: Some(walker.cursor),
                                        next_byte: 0,
                                        next_line: 1,
                                        content_sha256: String::new(),
                                    }),
                                    NodeKind::Directory => GrepPosition::Tree(walker.cursor),
                                    NodeKind::Symlink => {
                                        if !self.diagnostic(
                                            full_path,
                                            DiagnosticKind::SymlinkSkipped,
                                            "symbolic link was not followed",
                                        ) {
                                            return Ok(Some(GrepPosition::Tree(checkpoint)));
                                        }
                                        GrepPosition::Tree(walker.cursor)
                                    }
                                    NodeKind::Other => {
                                        if !self.diagnostic(
                                            full_path,
                                            DiagnosticKind::Unsupported,
                                            "unsupported filesystem entry was skipped",
                                        ) {
                                            return Ok(Some(GrepPosition::Tree(checkpoint)));
                                        }
                                        GrepPosition::Tree(walker.cursor)
                                    }
                                }
                            }
                        }
                        Ok(None) => return Ok(None),
                        Err(error) => {
                            if error.budget_exhausted {
                                return Ok(Some(GrepPosition::Tree(walker.cursor)));
                            }
                            if !self.walker_diagnostic(&error) {
                                return Ok(Some(GrepPosition::Tree(checkpoint)));
                            }
                            walker.cursor.frames.pop();
                            if walker.cursor.frames.is_empty() {
                                return Ok(None);
                            }
                            GrepPosition::Tree(walker.cursor)
                        }
                    }
                }
            };
        }
    }

    fn prune_excluded_ancestors(&self, mut tree: TreeCursor) -> TreeCursor {
        let mut relative = String::new();
        for (index, frame) in tree
            .frames
            .iter()
            .take(tree.frames.len().saturating_sub(1))
            .enumerate()
        {
            let Some(name) = &frame.after else {
                break;
            };
            if !relative.is_empty() {
                relative.push('/');
            }
            relative.push_str(name);
            if self.excludes.is_match(&relative) {
                tree.frames.truncate(index + 1);
                break;
            }
        }
        tree
    }

    async fn search_file(
        &mut self,
        cursor: GrepFileCursor,
    ) -> Result<FileProgress, FilesystemToolError> {
        if self.files_read >= MAX_FILES_PER_CALL {
            return Ok(FileProgress::Continue(cursor));
        }
        self.files_read += 1;
        let (display_path, relative, names) = self.active_file_location(&cursor)?;
        let mut ancestor = String::new();
        let excluded_ancestor = names[..names.len().saturating_sub(1)].iter().any(|name| {
            if !ancestor.is_empty() {
                ancestor.push('/');
            }
            ancestor.push_str(name);
            self.excludes.is_match(&ancestor)
        });
        if excluded_ancestor
            || self.excludes.is_match(&relative)
            || (!self.query.include_globs.is_empty() && !self.includes.is_match(&relative))
        {
            return Ok(FileProgress::Done(cursor.tree));
        }
        let node = match self.open_file_node(&names).await {
            Ok(node) => node,
            Err(error) if !cursor.content_sha256.is_empty() => {
                return Err(FilesystemToolError::InvalidCursor(format!(
                    "cannot reopen continued file '{}': {}",
                    display_path,
                    fs_error_message(&error)
                )));
            }
            Err(error) => {
                if !self.diagnostic(display_path, DiagnosticKind::Io, &fs_error_message(&error)) {
                    return Ok(FileProgress::Continue(cursor));
                }
                return Ok(FileProgress::Done(cursor.tree));
            }
        };
        let NodeKind::File(size) = node.kind() else {
            if !cursor.content_sha256.is_empty() {
                return self.changed_file(cursor, display_path);
            }
            if !self.diagnostic(
                display_path,
                DiagnosticKind::Unsupported,
                "selected entry is no longer a regular file",
            ) {
                return Ok(FileProgress::Continue(cursor));
            }
            return Ok(FileProgress::Done(cursor.tree));
        };
        if size > MAX_FILE_BYTES as u64 {
            if !cursor.content_sha256.is_empty() {
                return Err(FilesystemToolError::InvalidCursor(format!(
                    "continued grep file '{}' changed size",
                    display_path
                )));
            }
            if !self.diagnostic(
                display_path,
                DiagnosticKind::FileTooLarge,
                "file exceeds the 1 MiB search limit",
            ) {
                return Ok(FileProgress::Continue(cursor));
            }
            return Ok(FileProgress::Done(cursor.tree));
        }
        let read_limit = usize::try_from(size.saturating_add(1))
            .unwrap_or(MAX_FILE_BYTES + 1)
            .min(MAX_FILE_BYTES + 1);
        if !reserve_read_budget(&mut self.bytes_read, read_limit) {
            return Ok(FileProgress::Continue(cursor));
        }
        let (bytes, at_eof) = match node.read_bounded(read_limit).await {
            Ok(result) => result,
            Err(error) => {
                if !cursor.content_sha256.is_empty() {
                    return Err(FilesystemToolError::InvalidCursor(format!(
                        "cannot read continued file '{}': {}",
                        display_path,
                        fs_error_message(&error)
                    )));
                }
                if !self.diagnostic(display_path, DiagnosticKind::Io, &fs_error_message(&error)) {
                    return Ok(FileProgress::Continue(cursor));
                }
                return Ok(FileProgress::Done(cursor.tree));
            }
        };
        refund_unused_read_budget(&mut self.bytes_read, read_limit, bytes.len());
        if !at_eof {
            if !cursor.content_sha256.is_empty() {
                return Err(FilesystemToolError::InvalidCursor(format!(
                    "continued grep file '{}' changed while being read",
                    display_path
                )));
            }
            if !self.diagnostic(
                display_path,
                DiagnosticKind::Io,
                "file changed while being read",
            ) {
                return Ok(FileProgress::Continue(cursor));
            }
            return Ok(FileProgress::Done(cursor.tree));
        }
        let hash = sha256_hex(&bytes);
        if !cursor.content_sha256.is_empty() && cursor.content_sha256 != hash {
            return Err(FilesystemToolError::InvalidCursor(format!(
                "file '{}' changed between grep pages",
                display_path
            )));
        }
        if bytes.len() > MAX_FILE_BYTES {
            if !self.diagnostic(
                display_path,
                DiagnosticKind::FileTooLarge,
                "file exceeds the 1 MiB search limit",
            ) {
                return Ok(FileProgress::Continue(cursor));
            }
            return Ok(FileProgress::Done(cursor.tree));
        }
        if bytes.contains(&0) {
            if !self.diagnostic(
                display_path,
                DiagnosticKind::Binary,
                "file contains a NUL byte",
            ) {
                return Ok(FileProgress::Continue(cursor));
            }
            return Ok(FileProgress::Done(cursor.tree));
        }
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(_) => {
                if !self.diagnostic(
                    display_path,
                    DiagnosticKind::Binary,
                    "file is not valid UTF-8",
                ) {
                    return Ok(FileProgress::Continue(cursor));
                }
                return Ok(FileProgress::Done(cursor.tree));
            }
        };
        let lines = line_spans(text);
        let start_index = validate_file_position(&cursor, &lines, bytes.len())?;
        for (index, &(start, end)) in lines.iter().enumerate().skip(start_index) {
            if self.matches.len() >= self.query.limit as usize
                || self.lines_read >= MAX_LINES_PER_CALL
                || self.diagnostics.len() >= MAX_DIAGNOSTICS
            {
                return Ok(FileProgress::Continue(GrepFileCursor {
                    tree: cursor.tree,
                    next_byte: start as u64,
                    next_line: index as u64 + 1,
                    content_sha256: hash,
                }));
            }
            self.lines_read += 1;
            let line = &text[start..end];
            if line.len() > MAX_LINE_BYTES {
                if !self.diagnostic(
                    display_path.clone(),
                    DiagnosticKind::LineTooLong,
                    &format!("line {} exceeds the 64 KiB search limit", index + 1),
                ) {
                    return Ok(FileProgress::Continue(GrepFileCursor {
                        tree: cursor.tree,
                        next_byte: start as u64,
                        next_line: index as u64 + 1,
                        content_sha256: hash,
                    }));
                }
                continue;
            }
            if self.regex.is_match(line) {
                let (returned, truncated) = truncate_utf8(line, MAX_MATCH_TEXT_BYTES);
                let result_size = display_path.len() + returned.len() + 32;
                if self.output_bytes + result_size > MAX_CONTENT_OUTPUT_BYTES {
                    return Ok(FileProgress::Continue(GrepFileCursor {
                        tree: cursor.tree,
                        next_byte: start as u64,
                        next_line: index as u64 + 1,
                        content_sha256: hash,
                    }));
                }
                self.output_bytes += result_size;
                self.matches.push(GrepMatch {
                    path: display_path.clone(),
                    line: index as u64 + 1,
                    text: returned.to_string(),
                    text_truncated: truncated,
                });
            }
        }
        Ok(FileProgress::Done(cursor.tree))
    }

    fn active_file_location(
        &self,
        cursor: &GrepFileCursor,
    ) -> Result<(String, String, Vec<String>), FilesystemToolError> {
        match &cursor.tree {
            None => {
                let basename = Path::new(&self.query.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&self.query.path)
                    .to_string();
                Ok((self.query.path.clone(), basename, Vec::new()))
            }
            Some(tree) => {
                validate_tree_cursor(tree, self.query.max_depth)?;
                let names = tree
                    .frames
                    .iter()
                    .filter_map(|frame| frame.after.clone())
                    .collect::<Vec<_>>();
                let relative = names.join("/");
                Ok((join_path(&self.query.path, &relative), relative, names))
            }
        }
    }

    async fn open_file_node(&self, names: &[String]) -> Result<FsNode, FsError> {
        let mut node = FsNode::open_root(&self.query.path).await?;
        for name in names {
            node = node.open_child(name).await?;
        }
        Ok(node)
    }

    fn changed_file(
        &self,
        _cursor: GrepFileCursor,
        path: String,
    ) -> Result<FileProgress, FilesystemToolError> {
        Err(FilesystemToolError::InvalidCursor(format!(
            "continued grep file '{path}' is no longer a regular file"
        )))
    }

    fn diagnostic(&mut self, path: String, kind: DiagnosticKind, message: &str) -> bool {
        push_diagnostic(
            &mut self.diagnostics,
            &mut self.output_bytes,
            path,
            kind,
            message,
        )
    }

    fn walker_diagnostic(&mut self, error: &WalkerError) -> bool {
        let kind = match error.error {
            FsError::DirectoryTooLarge => DiagnosticKind::DirectoryTooLarge,
            FsError::Symlink => DiagnosticKind::SymlinkSkipped,
            _ => DiagnosticKind::Io,
        };
        self.diagnostic(error.path.clone(), kind, &fs_error_message(&error.error))
    }
}

enum FileProgress {
    Continue(GrepFileCursor),
    Done(Option<TreeCursor>),
}

fn validate_pattern(pattern: &str, mode: SearchMode) -> Result<(), FilesystemToolError> {
    if pattern.is_empty() {
        return Err(FilesystemToolError::InvalidPattern(
            "grep pattern must not be empty".to_string(),
        ));
    }
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err(FilesystemToolError::InvalidPattern(format!(
            "grep pattern exceeds {MAX_PATTERN_BYTES} bytes"
        )));
    }
    if mode == SearchMode::Literal && pattern.contains('\n') {
        return Err(FilesystemToolError::InvalidPattern(
            "literal grep patterns cannot contain LF".to_string(),
        ));
    }
    Ok(())
}

fn compile_regex(query: &GrepQuery) -> Result<Regex, FilesystemToolError> {
    let pattern = match query.mode {
        SearchMode::Literal => regex::escape(&query.pattern),
        SearchMode::Regex => query.pattern.clone(),
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!query.case_sensitive)
        .size_limit(2 * 1024 * 1024)
        .dfa_size_limit(2 * 1024 * 1024)
        .nest_limit(64)
        .build()
        .map_err(|error| FilesystemToolError::InvalidPattern(error.to_string()))
}

fn validate_grep_cursor(cursor: &GrepCursor, query: &GrepQuery) -> Result<(), FilesystemToolError> {
    if &cursor.query != query {
        return Err(FilesystemToolError::InvalidCursor(
            "grep cursor query does not match the invocation".to_string(),
        ));
    }
    match (&cursor.tree, &cursor.file) {
        (Some(tree), None) => validate_tree_cursor(tree, query.max_depth),
        (None, Some(file)) => {
            if file.next_line == 0 || file.content_sha256.len() > 64 {
                return Err(FilesystemToolError::InvalidCursor(
                    "grep file cursor has invalid line or hash".to_string(),
                ));
            }
            if file.content_sha256.is_empty() && (file.next_byte != 0 || file.next_line != 1) {
                return Err(FilesystemToolError::InvalidCursor(
                    "an unstarted grep file cursor must begin at byte zero and line one"
                        .to_string(),
                ));
            }
            if let Some(tree) = &file.tree {
                validate_tree_cursor(tree, query.max_depth)?;
                if tree
                    .frames
                    .last()
                    .and_then(|frame| frame.after.as_ref())
                    .is_none()
                {
                    return Err(FilesystemToolError::InvalidCursor(
                        "grep file cursor does not identify a file".to_string(),
                    ));
                }
            }
            Ok(())
        }
        _ => Err(FilesystemToolError::InvalidCursor(
            "grep cursor must contain exactly one tree or file position".to_string(),
        )),
    }?;
    if estimated_grep_cursor_bytes(cursor) > MAX_CURSOR_BYTES {
        return Err(FilesystemToolError::InvalidCursor(
            "grep cursor exceeds the complete cursor size limit".to_string(),
        ));
    }
    Ok(())
}

fn estimated_grep_query_bytes(query: &GrepQuery) -> usize {
    query.path.len()
        + query.pattern.len()
        + query
            .include_globs
            .iter()
            .chain(&query.exclude_globs)
            .map(String::len)
            .sum::<usize>()
        + 64
}

fn estimated_grep_cursor_bytes(cursor: &GrepCursor) -> usize {
    let position = cursor.tree.as_ref().map_or(0, estimated_tree_cursor_bytes)
        + cursor.file.as_ref().map_or(0, |file| {
            file.content_sha256.len()
                + file.tree.as_ref().map_or(0, estimated_tree_cursor_bytes)
                + 24
        });
    estimated_grep_query_bytes(&cursor.query) + position
}

fn validate_file_position(
    cursor: &GrepFileCursor,
    lines: &[(usize, usize)],
    file_len: usize,
) -> Result<usize, FilesystemToolError> {
    if cursor.next_byte == file_len as u64 && cursor.next_line == lines.len() as u64 + 1 {
        return Ok(lines.len());
    }
    let index = usize::try_from(cursor.next_line.checked_sub(1).ok_or_else(|| {
        FilesystemToolError::InvalidCursor("grep line must be one-based".to_string())
    })?)
    .map_err(|_| {
        FilesystemToolError::InvalidCursor("grep line does not fit the target index".to_string())
    })?;
    if lines
        .get(index)
        .is_none_or(|(start, _)| *start as u64 != cursor.next_byte)
    {
        return Err(FilesystemToolError::InvalidCursor(
            "grep cursor is not at the declared logical line boundary".to_string(),
        ));
    }
    Ok(index)
}

fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0;
    for (offset, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            let mut end = offset;
            if end > start && bytes[end - 1] == b'\r' {
                end -= 1;
            }
            spans.push((start, end));
            start = offset + 1;
        }
    }
    if start < bytes.len() {
        spans.push((start, bytes.len()));
    }
    spans
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut result = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(result, "{byte:02x}");
    }
    result
}

fn reserve_read_budget(consumed: &mut usize, requested: usize) -> bool {
    if consumed.saturating_add(requested) > MAX_BYTES_PER_CALL {
        return false;
    }
    *consumed += requested;
    true
}

fn refund_unused_read_budget(consumed: &mut usize, reserved: usize, actual: usize) {
    *consumed -= reserved.saturating_sub(actual);
}
