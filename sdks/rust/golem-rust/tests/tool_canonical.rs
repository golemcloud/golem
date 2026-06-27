// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Canonical agent-tools examples (§5.3.1 `grep`, §5.3.5.1 `git`) ported to the
//! Golem Rust SDK and validated through the public discover-tools registry path
//! (`get_all_tools` / `get_tool_by_name` / `get_extended_tool_by_name`, the same
//! functions `guest.discover-tools` / `guest.get-tool` delegate to).
//!
//! Adaptations from the spec's illustrative `golem_tool` crate to the finalized
//! SDK + WIT model:
//! - `Path` -> `std::path::PathBuf`, `Url` -> `url::Url`, `DateTime` ->
//!   `chrono::DateTime<Utc>`, `RegexString` -> `String` refined with
//!   `#[arg(regex = ...)]`.
//! - `keyvalue-t<string>` is `SchemaValue::Map`, authored as
//!   `BTreeMap<String, String>` (a repeatable-map option).
//! - color/output sum types are real enums deriving `IntoSchema`/`FromSchema`.
//! - Tool-wide globals propagate from an ancestor command; a pure-dispatcher
//!   root cannot carry globals, so multi-level propagation is exercised on the
//!   `git remote` subtree dispatcher (declared once, effective in descendants).

test_r::enable!();

// Both canonical examples register their tools into the same process-global
// tool registry, which is not thread-safe (it transitively holds a wit-bindgen
// guest resource handle and is built for the single-threaded WASM guest). A
// single parent `#[sequential]` suite makes test-r share one execution lock
// across every descendant test, so the two example modules never touch the
// registry concurrently *in-process*.
//
// Every test additionally carries `#[test_r::never_capture]`. With output
// capturing on, test-r runs the suite across parallel worker subprocesses
// (capture requires it), and that worker/IPC path is unreliable for this
// registry-reading binary. `never_capture` keeps the whole binary running
// in-process, where the `#[sequential]` lock fully serializes registry access.
// Any test added to this file MUST keep `#[test_r::never_capture]`, otherwise
// capturing turns back on for the binary and the flaky worker path returns.
#[cfg(test)]
#[cfg(feature = "export_golem_agentic")]
#[test_r::sequential]
mod canonical {
    #[allow(clippy::disallowed_names, dead_code)]
    mod grep_canonical {
        use golem_rust::agentic::{
            EffectiveCommandField, ExtendedOptionShape, ExtendedToolType,
            encode_schema_value_default, get_all_tools, get_extended_tool_by_name,
            get_tool_by_name, option_collected_graph, render_argument_help, render_help,
        };
        use golem_rust::schema::schema_type::NumericBound;
        use golem_rust::schema::{SchemaType, SchemaValue};
        use golem_rust::wasip2::io::streams::{InputStream, OutputStream};
        use golem_rust::{FromSchema, IntoSchema, tool_definition, tool_implementation};
        use golem_rust_macro::ToolError;
        use std::path::PathBuf;
        use test_r::test;

        #[derive(Clone, IntoSchema, FromSchema)]
        #[schema(rename_all = "kebab-case")]
        enum ColorMode {
            Always,
            Never,
            Auto,
        }

        #[derive(IntoSchema, FromSchema)]
        struct Hit {
            file: PathBuf,
            line: u32,
            text: String,
        }

        /// Errors raised by `grep`.
        #[derive(ToolError)]
        enum GrepError {
            /// The supplied pattern is not a valid regex.
            #[tool_error(kind = "usage-error", exit_code = 2)]
            InvalidPattern { reason: String },
            /// No line matched.
            #[tool_error(kind = "runtime-error", exit_code = 1)]
            NoMatch,
        }

        /// Search files for a regex pattern.
        #[tool_definition(version = "2.0.0")]
        trait Grep {
            /// Search files for a regex pattern. Bare `grep` runs this body.
            #[arg(case_sensitive = "global", short = 'i', kind = "flag")]
            #[arg(color = "global", default = "auto")]
            #[arg(pattern = "positional", regex = r"^.+$")]
            #[arg(
                extra_patterns = "option",
                short = 'e',
                repeatable = "either",
                delim = ','
            )]
            #[arg(max_count = "option", short = 'n', min = 1)]
            #[arg(files = "tail", accepts_stdio = true)]
            fn grep(
                &self,
                case_sensitive: bool,
                color: ColorMode,
                pattern: String,
                extra_patterns: Vec<String>,
                max_count: Option<u32>,
                files: Vec<PathBuf>,
                stdin: InputStream,
                stdout: OutputStream,
            ) -> Result<Vec<Hit>, GrepError>;

            /// In-place text replacement.
            fn replace(
                &self,
                case_sensitive: bool,
                color: ColorMode,
                pattern: String,
                replacement: String,
                files: Vec<PathBuf>,
            ) -> Result<u64, GrepError>;
        }

        struct LeafGrep;

        #[tool_implementation]
        impl Grep for LeafGrep {
            fn grep(
                &self,
                _case_sensitive: bool,
                _color: ColorMode,
                _pattern: String,
                _extra_patterns: Vec<String>,
                _max_count: Option<u32>,
                _files: Vec<PathBuf>,
                _stdin: InputStream,
                _stdout: OutputStream,
            ) -> Result<Vec<Hit>, GrepError> {
                Ok(vec![])
            }

            fn replace(
                &self,
                _case_sensitive: bool,
                _color: ColorMode,
                _pattern: String,
                _replacement: String,
                _files: Vec<PathBuf>,
            ) -> Result<u64, GrepError> {
                Ok(0)
            }
        }

        fn grep_tool() -> ExtendedToolType {
            get_extended_tool_by_name("grep")
                .expect("grep is registered via #[tool_implementation]")
        }

        #[test]
        #[test_r::never_capture]
        fn grep_is_discoverable_through_registry() {
            // `discover-tools` -> `get_all_tools`.
            let all = get_all_tools();
            assert!(
                all.iter().any(|t| t.commands.nodes[0].name == "grep"),
                "grep must appear in discover-tools output"
            );
            // `get-tool` -> `get_tool_by_name`.
            let wire = get_tool_by_name("grep").expect("get-tool resolves grep");
            assert_eq!(wire.version, "2.0.0");
            assert_eq!(wire.commands.nodes[0].name, "grep");
        }

        #[test]
        #[test_r::never_capture]
        fn grep_root_metadata() {
            let tool = grep_tool();
            assert_eq!(tool.version, "2.0.0");
            assert_eq!(tool.tool_name(), "grep");

            let root = &tool.commands[0];
            assert_eq!(root.name, "grep");

            // Two globals: a `color` option and a `case-sensitive` flag.
            assert_eq!(root.globals.options.len(), 1);
            let color = &root.globals.options[0];
            assert_eq!(color.long, "color");
            assert!(matches!(color.shape, ExtendedOptionShape::Scalar(_)));
            assert_eq!(root.globals.flags.len(), 1);
            assert_eq!(root.globals.flags[0].long, "case-sensitive");
            assert_eq!(root.globals.flags[0].short, Some('i'));

            let body = root.body.as_ref().expect("grep has an implicit body");

            // `pattern` positional carries the regex refinement.
            assert_eq!(body.positionals.fixed.len(), 1);
            assert_eq!(body.positionals.fixed[0].name, "pattern");

            // `files` is the stdio-bound tail positional.
            let tail = body.positionals.tail.as_ref().expect("files tail");
            assert_eq!(tail.name, "files");
            assert!(tail.accepts_stdio);

            // `extra-patterns` is a repeatable-list option; `max-count` a scalar.
            let extra = body
                .options
                .iter()
                .find(|o| o.long == "extra-patterns")
                .expect("extra-patterns option");
            assert_eq!(extra.short, Some('e'));
            assert!(matches!(
                extra.shape,
                ExtendedOptionShape::RepeatableList(_)
            ));
            let max_count = body
                .options
                .iter()
                .find(|o| o.long == "max-count")
                .expect("max-count option");
            assert_eq!(max_count.short, Some('n'));
            // `Option<u32>` makes the scalar option not-required (the distinct
            // `OptionalScalar` shape is opted into with `#[arg(optional_scalar)]`).
            assert!(matches!(max_count.shape, ExtendedOptionShape::Scalar(_)));
            assert!(!max_count.required);

            // stdin/stdout slots are present.
            assert!(body.stdin.is_some());
            assert!(body.stdout.is_some());

            // Two declared error cases (mixed usage/runtime kinds).
            assert_eq!(body.errors.len(), 2);
            assert_eq!(body.errors[0].name, "invalid-pattern");
            assert_eq!(body.errors[0].exit_code, 2);
            assert_eq!(body.errors[1].name, "no-match");
            assert_eq!(body.errors[1].exit_code, 1);

            tool.try_to_tool()
                .expect("grep tool is valid wire metadata");
        }

        #[test]
        #[test_r::never_capture]
        fn grep_replace_subcommand_deprojects_inherited_globals() {
            let tool = grep_tool();
            let root = &tool.commands[0];
            let replace = root
                .subcommands
                .iter()
                .map(|&i| &tool.commands[i as usize])
                .find(|c| c.name == "replace")
                .expect("replace subcommand");
            let body = replace.body.as_ref().expect("replace body");

            // `case_sensitive` / `color` repeat the inherited root globals and are
            // de-projected from the replace body (covered once, by the root global).
            assert!(
                !body.flags.iter().any(|f| f.long == "case-sensitive"),
                "inherited case-sensitive global must be suppressed in replace body"
            );
            assert!(
                !body.options.iter().any(|o| o.long == "color")
                    && !body.positionals.fixed.iter().any(|p| p.name == "color"),
                "inherited color global must be suppressed in replace body"
            );

            // The genuine body positionals remain, in order.
            let names: Vec<&str> = body
                .positionals
                .fixed
                .iter()
                .map(|p| p.name.as_str())
                .collect();
            assert_eq!(names, vec!["pattern", "replacement"]);
            let tail = body.positionals.tail.as_ref().expect("replace files tail");
            assert_eq!(tail.name, "files");

            tool.try_to_tool().expect("grep tool with replace is valid");
        }

        #[test]
        #[test_r::never_capture]
        fn grep_canonical_input_field_order() {
            // D7: globals (in root->node order: options then flags), then body
            // positionals (fixed, tail), options, flags.
            let tool = grep_tool();
            let fields = tool.canonical_input_fields(0);
            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(
                names,
                vec![
                    "color",
                    "case-sensitive",
                    "pattern",
                    "files",
                    "extra-patterns",
                    "max-count",
                ]
            );
        }

        #[test]
        #[test_r::never_capture]
        fn grep_color_default_encodes_to_schema_value_tree() {
            // D9: the `default = "auto"` literal resolves to the enum case and
            // encodes into a self-contained schema-value-tree.
            let tool = grep_tool();
            let color = tool.commands[0]
                .globals
                .options
                .iter()
                .find(|o| o.long == "color")
                .expect("color global option");
            let default = color.default.as_ref().expect("color has a default");
            assert_eq!(*default, SchemaValue::Enum { case: 2 });
            encode_schema_value_default(default).expect("default encodes to a schema-value-tree");
        }

        #[test]
        #[test_r::never_capture]
        fn grep_help_renders_at_root_and_argument_depth() {
            let tool = grep_tool();

            // The root command is addressed by the empty path; subcommands by their
            // names below the root.
            let root_help = render_help(&tool, &[]).expect("root help");
            assert!(root_help.contains("Usage: grep"));
            assert!(root_help.contains("Globals:"));
            assert!(root_help.contains("--color"));
            assert!(root_help.contains("--case-sensitive"));
            assert!(root_help.contains("pattern"));
            assert!(root_help.contains("files..."));
            assert!(root_help.contains("Subcommands:"));
            assert!(root_help.contains("replace"));

            let sub_help = render_help(&tool, &["replace".to_string()]).expect("sub help");
            assert!(sub_help.contains("Usage: replace"));

            let color_help = render_argument_help(&tool, &[], "color").expect("color arg help");
            assert!(color_help.contains("--color (option, global)"));

            let pattern_help =
                render_argument_help(&tool, &[], "pattern").expect("pattern arg help");
            assert!(pattern_help.contains("pattern (positional"));
        }

        // --- D8: globals declared once on an ancestor are effective in descendants -

        #[test]
        #[test_r::never_capture]
        fn grep_global_is_effective_on_subcommand() {
            let tool = grep_tool();
            let replace_idx = tool.commands[0]
                .subcommands
                .iter()
                .copied()
                .find(|&i| tool.commands[i as usize].name == "replace")
                .expect("replace index") as usize;

            // The replace node stores no globals itself...
            assert!(tool.commands[replace_idx].globals.options.is_empty());
            assert!(tool.commands[replace_idx].globals.flags.is_empty());

            // ...but the root globals are effective at replace depth.
            let effective = tool.effective_globals(replace_idx);
            assert!(effective.iter().any(|g| matches!(
                g,
                EffectiveCommandField::Flag(f) if f.long == "case-sensitive"
            )));
            assert!(effective.iter().any(|g| matches!(
                g,
                EffectiveCommandField::Option(o) if o.long == "color"
            )));
        }

        // --- D1: a tool with no #[tool_implementation] is never registered ---------

        /// A defined-but-unimplemented tool. No `#[tool_implementation]` impl exists,
        /// so no ctor registers it.
        #[tool_definition]
        trait Unimplemented {
            fn unimplemented(&self, name: String) -> Result<(), GrepError>;
        }

        #[test]
        #[test_r::never_capture]
        fn unimplemented_tool_is_not_registered() {
            assert!(
                get_extended_tool_by_name("unimplemented").is_none(),
                "a tool with no #[tool_implementation] must not be discoverable"
            );
            // It must also be absent from the full discover-tools listing.
            assert!(
                !get_all_tools()
                    .iter()
                    .any(|t| t.commands.nodes[0].name == "unimplemented")
            );
        }

        // --- D2: numeric bounds up to u64::MAX -------------------------------------

        #[tool_definition]
        trait BigBound {
            #[arg(count = "option", bounds = (0, u64::MAX))]
            fn big_bound(&self, count: Option<u64>) -> Result<(), GrepError>;
        }

        struct LeafBigBound;

        #[tool_implementation]
        impl BigBound for LeafBigBound {
            fn big_bound(&self, _count: Option<u64>) -> Result<(), GrepError> {
                Ok(())
            }
        }

        #[test]
        #[test_r::never_capture]
        fn numeric_bounds_reach_u64_max() {
            // Through the discover-tools path: registered via #[tool_implementation],
            // resolved by name, and round-tripped to wire metadata.
            get_tool_by_name("big-bound").expect("get-tool resolves big-bound");
            let tool = get_extended_tool_by_name("big-bound")
                .expect("big-bound is registered via #[tool_implementation]");
            let body = tool.commands[0].body.as_ref().expect("body");
            let count = body
                .options
                .iter()
                .find(|o| o.long == "count")
                .expect("count option");
            let graph = option_collected_graph(&count.shape);
            match &graph.root {
                SchemaType::U64 { restrictions, .. } => {
                    let r = restrictions.as_ref().expect("u64 restrictions present");
                    assert_eq!(r.min, Some(NumericBound::Unsigned(0)));
                    assert_eq!(r.max, Some(NumericBound::Unsigned(u64::MAX)));
                }
                other => panic!("expected a U64 option type, got {other:?}"),
            }
            tool.try_to_tool()
                .expect("u64::MAX bound encodes to valid wire metadata");
        }
    }

    // The canonical `git` subset (§5.3.5.1). It needs the `url` and `chrono`
    // schema features for the `Url` / `DateTime<Utc>` nodes, so it is compiled only
    // when those features are enabled (in addition to `export_golem_agentic`).
    #[cfg(all(feature = "url", feature = "chrono"))]
    #[allow(clippy::disallowed_names, dead_code)]
    mod git_canonical {
        use chrono::{DateTime, Utc};
        use golem_rust::agentic::{
            EffectiveCommandField, ExtendedConstraint, ExtendedOptionShape, ExtendedRef,
            ExtendedRepeatableListShape, ExtendedToolType, ExtendedValueIsLiteral,
            get_extended_tool_by_name, get_tool_by_name, option_collected_graph,
        };
        use golem_rust::golem_agentic::golem::tool::common::{ErrorKind, FlagShape, Repetition};
        use golem_rust::schema::{SchemaType, SchemaValue};
        use golem_rust::{FromSchema, IntoSchema, tool_definition, tool_implementation};
        use golem_rust_macro::ToolError;
        use std::collections::BTreeMap;
        use std::path::PathBuf;
        use test_r::test;
        use url::Url;

        #[derive(Clone, IntoSchema, FromSchema)]
        #[schema(rename_all = "kebab-case")]
        enum OutputMode {
            Human,
            Porcelain,
            Json,
        }

        #[derive(IntoSchema, FromSchema)]
        struct CommitResult {
            hash: String,
            files_changed: u32,
            insertions: u32,
            deletions: u32,
        }

        #[derive(IntoSchema, FromSchema)]
        struct LogEntry {
            hash: String,
            author: String,
            date: DateTime<Utc>,
            message: String,
        }

        #[derive(ToolError)]
        enum CommitError {
            #[tool_error(kind = "runtime-error", exit_code = 1)]
            NothingStaged,
            #[tool_error(kind = "runtime-error", exit_code = 128)]
            DirtyMerge,
            #[tool_error(kind = "usage-error", exit_code = 129)]
            BadAuthorFormat { author: String },
        }

        #[derive(ToolError)]
        enum LogError {
            #[tool_error(kind = "usage-error", exit_code = 128)]
            BadRevision,
            #[tool_error(kind = "usage-error", exit_code = 129)]
            NotARepository,
        }

        #[derive(ToolError)]
        enum RemoteError {
            #[tool_error(kind = "usage-error", exit_code = 128)]
            NoSuchRemote { name: String },
        }

        #[derive(ToolError)]
        enum SetUrlError {
            #[tool_error(kind = "runtime-error", exit_code = 1)]
            Failed(String),
        }

        /// Stupid content tracker.
        #[tool_definition]
        trait Git {
            /// Record changes to the repository.
            #[command(aliases = ["ci"], annotations(destructive = true))]
            // Globals effective on this command:
            #[arg(verbose = "global", short = 'v', kind = "count-flag", max = 3)]
            #[arg(git_dir = "global", env = "GIT_DIR", default = ".git")]
            #[arg(paginate = "global", kind = "flag", negatable = true, default = true)]
            #[arg(config = "global", short = 'c', repeatable = "repeated")]
            // Per-command:
            #[arg(message = "option", short = 'm', required = true, aliases = ["msg"])]
            #[arg(author = "option", env = "GIT_AUTHOR_NAME", regex = r"^.+ <.+@.+>$")]
            #[arg(amend = "flag", negatable = true, default = false)]
            #[arg(signoff = "flag", negatable = true, default = false)]
            #[arg(reset_author = "flag", default = false)]
            #[arg(output = "option", default = "human")]
            #[constraint(implies(lhs = "reset-author", rhs = "amend"))]
            #[constraint(requires_all = value_is("output", "json"))]
            #[result(formatters = ["human", "porcelain", "json"], default = "human")]
            fn commit(
                &self,
                verbose: u32,
                git_dir: PathBuf,
                paginate: bool,
                config: BTreeMap<String, String>,
                message: String,
                author: Option<String>,
                amend: bool,
                signoff: bool,
                reset_author: bool,
                output: OutputMode,
            ) -> Result<CommitResult, CommitError>;

            /// Manage set of tracked repositories. Pure dispatcher.
            // The subtree dispatcher declares the propagating globals once; they are
            // effective in every descendant of `git remote`.
            #[command(subtree = Remote, aliases = ["rmt"])]
            #[arg(verbose = "global", short = 'v', kind = "count-flag", max = 3)]
            #[arg(git_dir = "global", env = "GIT_DIR", default = ".git")]
            #[arg(paginate = "global", kind = "flag", negatable = true, default = true)]
            #[arg(config = "global", short = 'c', repeatable = "repeated")]
            fn remote(
                &self,
                verbose: u32,
                git_dir: PathBuf,
                paginate: bool,
                config: BTreeMap<String, String>,
            ) -> RemoteSubtree;

            /// Show commit logs.
            #[command(annotations(read_only = true, idempotent = true))]
            #[arg(max_count = "option", short = 'n', bounds = (0, i64::MAX))]
            #[arg(since = "option")]
            #[arg(until = "option")]
            #[arg(author = "option", repeatable = "delimited", delim = ',')]
            #[arg(grep = "option", repeatable = "either", delim = ',')]
            #[arg(all_match = "flag")]
            #[arg(invert_grep = "flag")]
            #[arg(oneline = "flag")]
            #[arg(graph = "flag")]
            #[arg(paths = "tail", separator = "--", min = 0)]
            #[constraint(all_or_none = ["all-match", "grep"])]
            #[result(formatters = ["oneline", "short", "medium", "full"], default = "medium")]
            fn log(
                &self,
                max_count: Option<i64>,
                since: Option<DateTime<Utc>>,
                until: Option<DateTime<Utc>>,
                author: Vec<String>,
                grep: Vec<String>,
                all_match: bool,
                invert_grep: bool,
                oneline: bool,
                graph: bool,
                paths: Vec<PathBuf>,
            ) -> Result<Vec<LogEntry>, LogError>;
        }

        /// Placeholder return type for the `remote` subtree dispatcher.
        struct RemoteSubtree;

        /// Subcommand subtree under `git remote`. Pure dispatcher.
        #[tool_definition]
        trait Remote {
            /// Add a remote.
            #[command(annotations(destructive = false, idempotent = false))]
            #[arg(name = "positional", regex = r"^[a-zA-Z][a-zA-Z0-9_-]*$")]
            #[arg(url = "positional")]
            #[arg(track = "option", short = 't', repeatable = "repeated")]
            #[arg(master = "option", short = 'm')]
            #[arg(tags = "flag", negatable = true, default = true)]
            #[arg(fetch = "flag", short = 'f', default = false)]
            // `verbose` repeats the inherited `remote` global; it is de-projected
            // from the body when grafted under `git remote`.
            #[arg(verbose, kind = "count-flag", max = 3)]
            fn add(
                &self,
                verbose: u32,
                name: String,
                url: Url,
                track: Vec<String>,
                master: Option<String>,
                tags: bool,
                fetch: bool,
            ) -> Result<(), RemoteError>;

            /// Remove a remote.
            #[command(aliases = ["rm"], annotations(destructive = true, idempotent = true))]
            #[arg(name = "positional", regex = r"^[a-zA-Z][a-zA-Z0-9_-]*$")]
            fn remove(&self, name: String) -> Result<(), RemoteError>;

            /// Change a remote URL.
            #[command(annotations(destructive = true))]
            #[arg(name = "positional")]
            #[arg(newurl = "positional")]
            #[arg(oldurl = "positional", required = false)]
            #[arg(push = "flag")]
            #[arg(add = "flag")]
            #[arg(delete = "flag")]
            #[constraint(mutex_groups = [["add"], ["delete"]])]
            fn set_url(
                &self,
                name: String,
                newurl: Url,
                oldurl: Option<Url>,
                push: bool,
                add: bool,
                delete: bool,
            ) -> Result<(), SetUrlError>;
        }

        struct LeafGit;

        #[tool_implementation]
        impl Git for LeafGit {
            fn commit(
                &self,
                _verbose: u32,
                _git_dir: PathBuf,
                _paginate: bool,
                _config: BTreeMap<String, String>,
                _message: String,
                _author: Option<String>,
                _amend: bool,
                _signoff: bool,
                _reset_author: bool,
                _output: OutputMode,
            ) -> Result<CommitResult, CommitError> {
                Ok(CommitResult {
                    hash: String::new(),
                    files_changed: 0,
                    insertions: 0,
                    deletions: 0,
                })
            }

            fn remote(
                &self,
                _verbose: u32,
                _git_dir: PathBuf,
                _paginate: bool,
                _config: BTreeMap<String, String>,
            ) -> RemoteSubtree {
                RemoteSubtree
            }

            fn log(
                &self,
                _max_count: Option<i64>,
                _since: Option<DateTime<Utc>>,
                _until: Option<DateTime<Utc>>,
                _author: Vec<String>,
                _grep: Vec<String>,
                _all_match: bool,
                _invert_grep: bool,
                _oneline: bool,
                _graph: bool,
                _paths: Vec<PathBuf>,
            ) -> Result<Vec<LogEntry>, LogError> {
                Ok(vec![])
            }
        }

        fn git_tool() -> ExtendedToolType {
            get_extended_tool_by_name("git").expect("git is registered via #[tool_implementation]")
        }

        fn child<'a>(
            tool: &'a ExtendedToolType,
            parent: usize,
            name: &str,
        ) -> &'a golem_rust::agentic::ExtendedCommandNode {
            let idx = tool.commands[parent]
                .subcommands
                .iter()
                .copied()
                .find(|&i| tool.commands[i as usize].name == name)
                .unwrap_or_else(|| panic!("missing subcommand {name}"))
                as usize;
            &tool.commands[idx]
        }

        fn child_index(tool: &ExtendedToolType, parent: usize, name: &str) -> usize {
            tool.commands[parent]
                .subcommands
                .iter()
                .copied()
                .find(|&i| tool.commands[i as usize].name == name)
                .unwrap_or_else(|| panic!("missing subcommand {name}")) as usize
        }

        #[test]
        #[test_r::never_capture]
        fn git_is_discoverable_with_pure_dispatcher_root() {
            let wire = get_tool_by_name("git").expect("get-tool resolves git");
            assert_eq!(wire.commands.nodes[0].name, "git");
            let tool = git_tool();
            let root = &tool.commands[0];
            assert_eq!(root.name, "git");
            // Pure dispatcher: no implicit body.
            assert!(root.body.is_none());
            // The subtree-only child `Remote` is grafted, not registered standalone.
            assert!(get_extended_tool_by_name("remote").is_none());
        }

        #[test]
        #[test_r::never_capture]
        fn git_top_level_subcommands_aliases_and_annotations() {
            let tool = git_tool();
            let names: Vec<&str> = tool.commands[0]
                .subcommands
                .iter()
                .map(|&i| tool.commands[i as usize].name.as_str())
                .collect();
            assert!(names.contains(&"commit"));
            assert!(names.contains(&"remote"));
            assert!(names.contains(&"log"));

            let commit = child(&tool, 0, "commit");
            assert_eq!(commit.aliases, vec!["ci".to_string()]);
            let commit_ann = commit
                .body
                .as_ref()
                .unwrap()
                .annotations
                .as_ref()
                .expect("commit annotations");
            assert!(commit_ann.destructive);

            let log = child(&tool, 0, "log");
            let log_ann = log
                .body
                .as_ref()
                .unwrap()
                .annotations
                .as_ref()
                .expect("log annotations");
            assert!(log_ann.read_only);
            assert!(log_ann.idempotent);

            // The `remote` dispatcher carries an alias but no body (pure dispatcher).
            let remote = child(&tool, 0, "remote");
            assert_eq!(remote.aliases, vec!["rmt".to_string()]);
            assert!(remote.body.is_none());
        }

        #[test]
        #[test_r::never_capture]
        fn git_commit_globals_options_constraint_and_formatters() {
            let tool = git_tool();
            let commit = child(&tool, 0, "commit");

            // Globals: count-flag + negatable flag (flags), git-dir + config (options).
            let verbose = commit
                .globals
                .flags
                .iter()
                .find(|f| f.long == "verbose")
                .expect("verbose count-flag global");
            assert_eq!(verbose.short, Some('v'));
            assert!(
                matches!(verbose.shape, FlagShape::CountFlag(Some(3))),
                "verbose is a count-flag capped at 3, got {:?}",
                verbose.shape
            );
            let paginate = commit
                .globals
                .flags
                .iter()
                .find(|f| f.long == "paginate")
                .expect("paginate flag global");
            match &paginate.shape {
                FlagShape::BoolFlag(b) => {
                    assert!(b.default, "paginate defaults to true");
                    assert!(b.negatable, "paginate is negatable");
                }
                other => panic!("expected paginate bool flag, got {other:?}"),
            }
            let git_dir = commit
                .globals
                .options
                .iter()
                .find(|o| o.long == "git-dir")
                .expect("git-dir option global");
            assert_eq!(git_dir.env_var.as_deref(), Some("GIT_DIR"));
            assert!(git_dir.default.is_some());
            let config = commit
                .globals
                .options
                .iter()
                .find(|o| o.long == "config")
                .expect("config option global");
            match &config.shape {
                ExtendedOptionShape::RepeatableMap(map) => {
                    assert!(matches!(map.repetition, Repetition::Repeated));
                    let root = map
                        .map_type
                        .resolve_ref(&map.map_type.root)
                        .expect("config map node resolves");
                    assert!(
                        matches!(root, SchemaType::Map { .. }),
                        "config is a Map node"
                    );
                }
                other => panic!("expected repeatable-map config, got {other:?}"),
            }

            let body = commit.body.as_ref().unwrap();
            let message = body
                .options
                .iter()
                .find(|o| o.long == "message")
                .expect("message option");
            assert_eq!(message.short, Some('m'));
            assert!(message.required);
            assert_eq!(message.aliases, vec!["msg".to_string()]);
            // `output` resolves to an enum node with its default case.
            let output = body
                .options
                .iter()
                .find(|o| o.long == "output")
                .expect("output option");
            let output_graph = option_collected_graph(&output.shape);
            let output_root = output_graph
                .resolve_ref(&output_graph.root)
                .expect("output enum resolves");
            assert!(matches!(output_root, SchemaType::Enum { .. }));
            assert!(output.default.is_some());

            // implies constraint present.
            assert!(
                body.constraints
                    .iter()
                    .any(|c| matches!(c, ExtendedConstraint::Implies(_))),
                "commit has an implies constraint"
            );
            // D9: a `value_is(...)` literal is resolved against the referenced
            // option's enum schema into a self-contained schema-value-tree.
            let value_is = body
                .constraints
                .iter()
                .find_map(|c| match c {
                    ExtendedConstraint::RequiresAll(refs) => refs.iter().find_map(|r| match r {
                        ExtendedRef::ValueIs(v) => Some(v),
                        ExtendedRef::Present(_) => None,
                    }),
                    _ => None,
                })
                .expect("commit has a value_is constraint");
            assert_eq!(value_is.name, "output");
            assert!(
                matches!(
                    &value_is.value,
                    ExtendedValueIsLiteral::Resolved(SchemaValue::Enum { case: 2 })
                ),
                "value_is(\"output\", \"json\") resolves to the json enum case, got {:?}",
                value_is.value
            );
            // Result formatters with a default.
            let result = body.result.as_ref().expect("commit result");
            let formatter_names: Vec<&str> =
                result.formatters.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(formatter_names, vec!["human", "porcelain", "json"]);
            assert_eq!(result.default_formatter, "human");
            // Mixed usage / runtime exit codes and error kinds.
            let codes: Vec<u8> = body.errors.iter().map(|e| e.exit_code).collect();
            assert!(codes.contains(&1));
            assert!(codes.contains(&128));
            assert!(codes.contains(&129));
            assert!(
                body.errors.iter().any(
                    |e| e.name == "nothing-staged" && matches!(e.kind, ErrorKind::RuntimeError)
                ),
                "nothing-staged is a runtime error"
            );
            assert!(
                body.errors
                    .iter()
                    .any(|e| e.name == "bad-author-format"
                        && matches!(e.kind, ErrorKind::UsageError)),
                "bad-author-format is a usage error"
            );
        }

        #[test]
        #[test_r::never_capture]
        fn git_log_bounds_datetime_repeatable_modes_tail_separator_and_constraint() {
            let tool = git_tool();
            let log = child(&tool, 0, "log");
            let body = log.body.as_ref().unwrap();

            // i64::MAX bound on max-count.
            let max_count = body
                .options
                .iter()
                .find(|o| o.long == "max-count")
                .expect("max-count");
            match option_collected_graph(&max_count.shape).root {
                SchemaType::S64 { restrictions, .. } => {
                    let r = restrictions.expect("s64 restrictions");
                    assert_eq!(
                        r.max,
                        Some(golem_rust::schema::schema_type::NumericBound::Signed(
                            i64::MAX
                        ))
                    );
                }
                other => panic!("expected S64 max-count, got {other:?}"),
            }

            // datetime-typed options.
            let since = body
                .options
                .iter()
                .find(|o| o.long == "since")
                .expect("since");
            assert!(matches!(
                option_collected_graph(&since.shape).root,
                SchemaType::Datetime { .. }
            ));

            // delimited vs either repeatable modes.
            let author = body
                .options
                .iter()
                .find(|o| o.long == "author")
                .expect("author");
            match &author.shape {
                ExtendedOptionShape::RepeatableList(ExtendedRepeatableListShape {
                    repetition,
                    ..
                }) => {
                    assert!(matches!(repetition, Repetition::Delimited(_)));
                }
                other => panic!("expected delimited repeatable author, got {other:?}"),
            }
            let grep = body
                .options
                .iter()
                .find(|o| o.long == "grep")
                .expect("grep");
            match &grep.shape {
                ExtendedOptionShape::RepeatableList(ExtendedRepeatableListShape {
                    repetition,
                    ..
                }) => {
                    assert!(matches!(repetition, Repetition::Either(_)));
                }
                other => panic!("expected either repeatable grep, got {other:?}"),
            }

            // tail positional with `--` separator.
            let tail = body.positionals.tail.as_ref().expect("paths tail");
            assert_eq!(tail.name, "paths");
            assert_eq!(tail.separator.as_deref(), Some("--"));

            // all-or-none constraint.
            assert!(
                body.constraints
                    .iter()
                    .any(|c| matches!(c, ExtendedConstraint::AllOrNone(_))),
                "log has an all-or-none constraint"
            );
        }

        #[test]
        #[test_r::never_capture]
        fn git_remote_subtree_multilevel_globals_and_deprojection() {
            let tool = git_tool();
            let remote_idx = child_index(&tool, 0, "remote");
            let remote = &tool.commands[remote_idx];

            // Globals declared ONCE on the remote dispatcher.
            assert!(remote.globals.flags.iter().any(|f| f.long == "verbose"));
            assert!(remote.globals.options.iter().any(|o| o.long == "git-dir"));

            // Children: add, rm (alias of remove), set-url.
            let sub_names: Vec<&str> = remote
                .subcommands
                .iter()
                .map(|&i| tool.commands[i as usize].name.as_str())
                .collect();
            assert!(sub_names.contains(&"add"));
            assert!(sub_names.contains(&"remove"));
            assert!(sub_names.contains(&"set-url"));

            // D8: globals are effective at depth (on `add`) though declared once.
            let add_idx = child_index(&tool, remote_idx, "add");
            let effective = tool.effective_globals(add_idx);
            assert!(effective.iter().any(|g| matches!(
                g,
                EffectiveCommandField::Flag(f) if f.long == "verbose"
            )));
            assert!(effective.iter().any(|g| matches!(
                g,
                EffectiveCommandField::Option(o) if o.long == "git-dir"
            )));

            // The `add` body re-declares `verbose`; it is de-projected (covered by
            // the inherited count-flag global).
            let add = &tool.commands[add_idx];
            let add_body = add.body.as_ref().unwrap();
            assert!(
                !add_body.flags.iter().any(|f| f.long == "verbose"),
                "inherited verbose global must be suppressed from the add body"
            );

            // url-typed positional on add.
            let url_pos = add_body
                .positionals
                .fixed
                .iter()
                .find(|p| p.name == "url")
                .expect("url positional");
            assert!(matches!(url_pos.type_.root, SchemaType::Url { .. }));

            // track repeatable-list option in `repeated` mode (--track a --track b).
            match &add_body
                .options
                .iter()
                .find(|o| o.long == "track")
                .expect("track")
                .shape
            {
                ExtendedOptionShape::RepeatableList(ExtendedRepeatableListShape {
                    repetition,
                    ..
                }) => assert!(matches!(repetition, Repetition::Repeated)),
                other => panic!("expected repeated repeatable track, got {other:?}"),
            }

            // `remove` carries the declared `rm` alias.
            let remove = child(&tool, remote_idx, "remove");
            assert_eq!(remove.aliases, vec!["rm".to_string()]);

            // D7 at subtree depth: the exact canonical field order is inherited
            // globals (options then flags, in the `remote` dispatcher's declaration
            // order), then this body's positionals, then options, then flags. The
            // re-declared `verbose` appears once, as the inherited count-flag global.
            let add_fields = tool.canonical_input_fields(add_idx);
            let names: Vec<&str> = add_fields.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(
                names,
                vec![
                    // inherited globals: options then flags
                    "git-dir", "config", "verbose", "paginate", // body positionals
                    "name", "url", // body options
                    "track", "master", // body flags
                    "tags", "fetch",
                ],
                "canonical input field order at subtree depth (D7)"
            );
        }

        #[test]
        #[test_r::never_capture]
        fn git_set_url_optional_trailing_positional_and_mutex() {
            let tool = git_tool();
            let remote_idx = child_index(&tool, 0, "remote");
            let set_url = child(&tool, remote_idx, "set-url");
            let body = set_url.body.as_ref().unwrap();

            let positionals: Vec<(&str, bool)> = body
                .positionals
                .fixed
                .iter()
                .map(|p| (p.name.as_str(), p.required))
                .collect();
            assert_eq!(
                positionals,
                vec![("name", true), ("newurl", true), ("oldurl", false)],
                "oldurl is the optional trailing positional"
            );

            assert!(
                body.constraints
                    .iter()
                    .any(|c| matches!(c, ExtendedConstraint::MutexGroups(_))),
                "set-url has a mutex-groups constraint"
            );
        }

        #[test]
        #[test_r::never_capture]
        fn git_builds_valid_wire_metadata() {
            git_tool()
                .try_to_tool()
                .expect("the full git tool is valid wire metadata");
        }
    }
}
