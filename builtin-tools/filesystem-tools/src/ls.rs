use crate::{FilesystemToolError, discovery::*};
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, WireSchema, tool_definition, tool_implementation,
};

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub enum EntryKind {
    File(u64),
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct LsEntry {
    /// Path in the same absolute or relative form as the requested root.
    pub path: String,
    /// Filesystem entry type and, for regular files, its byte size.
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct LsQuery {
    pub path: String,
    pub max_depth: u32,
    pub glob: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct LsCursor {
    /// Effective query arguments. They must match the continuation call.
    pub query: LsQuery,
    /// Root-relative traversal checkpoint.
    pub tree: TreeCursor,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct LsResult {
    pub entries: Vec<LsEntry>,
    pub diagnostics: Vec<DiscoveryDiagnostic>,
    pub next_cursor: Option<LsCursor>,
}

#[tool_definition(version = "0.1.0", requires_filesystem = true)]
pub trait Ls {
    /// Lists a directory in deterministic depth-first order without following symbolic links.
    /// The selected directory is depth zero; the default maximum depth lists its immediate
    /// children. Results and filesystem work are bounded, and `next_cursor` continues the same
    /// effective query. The optional glob matches slash-separated paths relative to the root.
    #[command(annotations(
        read_only = true,
        destructive = false,
        idempotent = true,
        open_world = false
    ))]
    async fn ls(
        &self,
        path: String,
        max_depth: Option<u32>,
        glob: Option<String>,
        limit: Option<u32>,
        cursor: Option<LsCursor>,
    ) -> Result<LsResult, FilesystemToolError>;
}

struct LsImpl;

#[tool_implementation]
impl Ls for LsImpl {
    async fn ls(
        &self,
        path: String,
        max_depth: Option<u32>,
        glob: Option<String>,
        limit: Option<u32>,
        cursor: Option<LsCursor>,
    ) -> Result<LsResult, FilesystemToolError> {
        list(path, max_depth, glob, limit, cursor).await
    }
}

pub async fn list(
    path: String,
    max_depth: Option<u32>,
    glob: Option<String>,
    limit: Option<u32>,
    cursor: Option<LsCursor>,
) -> Result<LsResult, FilesystemToolError> {
    validate_discovery_path(&path)?;
    if let Some(glob) = glob.as_ref() {
        validate_glob_inputs(std::slice::from_ref(glob), &[])?;
    }
    let query = LsQuery {
        path: path.clone(),
        max_depth: effective_depth(max_depth)?,
        glob,
        limit: effective_limit(limit)?,
    };
    let matcher = query
        .glob
        .as_ref()
        .map(|pattern| compile_globs(std::slice::from_ref(pattern)))
        .transpose()?;
    let root = open_command_root(&path).await?;
    if !matches!(root.kind(), NodeKind::Directory) {
        return Err(FilesystemToolError::NotADirectory(path));
    }
    let tree = match cursor {
        Some(cursor) => {
            validate_ls_cursor(&cursor, &query)?;
            cursor.tree
        }
        None => TreeCursor {
            frames: vec![DirectoryFrame { after: None }],
        },
    };
    let mut walker = TreeWalker::new(path.clone(), query.max_depth, tree);
    let mut entries = Vec::new();
    let mut diagnostics = Vec::new();
    let mut output_bytes = 0usize;
    let mut processed = 0usize;
    let mut remaining_entries = MAX_ENTRIES_EXAMINED;
    while entries.len() < query.limit as usize && processed < MAX_TRAVERSAL_ENTRIES {
        let checkpoint = walker.cursor.clone();
        match walker.next(|_| true, &mut remaining_entries).await {
            Ok(Some((relative, kind))) => {
                processed += 1;
                let full_path = join_path(&path, &relative);
                if full_path.len() > MAX_PATH_BYTES {
                    if !push_diagnostic(
                        &mut diagnostics,
                        &mut output_bytes,
                        full_path,
                        DiagnosticKind::PathTooLong,
                        "entry path exceeds the 4 KiB limit",
                    ) {
                        walker.cursor = checkpoint;
                        break;
                    }
                    continue;
                }
                if matcher
                    .as_ref()
                    .is_some_and(|matcher| !matcher.is_match(&relative))
                {
                    continue;
                }
                let entry = LsEntry {
                    path: full_path,
                    kind: public_kind(kind),
                };
                let size = entry.path.len() + 32;
                if output_bytes + size > MAX_CONTENT_OUTPUT_BYTES {
                    walker.cursor = checkpoint;
                    break;
                }
                output_bytes += size;
                entries.push(entry);
            }
            Ok(None) => break,
            Err(error) => {
                if error.budget_exhausted {
                    break;
                }
                if !push_walker_diagnostic(
                    &mut diagnostics,
                    &mut output_bytes,
                    &error,
                    DiagnosticKind::Io,
                ) {
                    walker.cursor = checkpoint;
                    break;
                }
                walker.cursor.frames.pop();
            }
        }
    }
    let next_cursor = (!walker.cursor.frames.is_empty()).then_some(LsCursor {
        query,
        tree: walker.cursor,
    });
    Ok(LsResult {
        entries,
        diagnostics,
        next_cursor,
    })
}

#[allow(clippy::too_many_arguments)]
fn validate_ls_cursor(cursor: &LsCursor, query: &LsQuery) -> Result<(), FilesystemToolError> {
    if &cursor.query != query {
        return Err(FilesystemToolError::InvalidCursor(
            "ls cursor query does not match the invocation".to_string(),
        ));
    }
    validate_tree_cursor(&cursor.tree, query.max_depth)?;
    if estimated_ls_cursor_bytes(cursor) > MAX_CURSOR_BYTES {
        return Err(FilesystemToolError::InvalidCursor(
            "ls cursor exceeds the complete cursor size limit".to_string(),
        ));
    }
    Ok(())
}

fn estimated_ls_cursor_bytes(cursor: &LsCursor) -> usize {
    cursor.query.path.len()
        + cursor.query.glob.as_ref().map_or(0, String::len)
        + estimated_tree_cursor_bytes(&cursor.tree)
        + 32
}

fn public_kind(kind: NodeKind) -> EntryKind {
    match kind {
        NodeKind::File(size) => EntryKind::File(size),
        NodeKind::Directory => EntryKind::Directory,
        NodeKind::Symlink => EntryKind::Symlink,
        NodeKind::Other => EntryKind::Other,
    }
}
