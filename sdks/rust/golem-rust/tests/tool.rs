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

test_r::enable!();

#[cfg(test)]
#[cfg(feature = "export_golem_agentic")]
#[test_r::sequential]
#[allow(clippy::disallowed_names, dead_code)]
mod tests {
    use golem_rust::agentic::{
        ExtendedOptionShape, ExtendedToolType, ToolBuildCtx, ToolBuildError, ToolErrorSchema,
    };
    use golem_rust::{
        FromSchema, IntoSchema, Quantity, QuantityUnit, tool_definition, tool_implementation,
    };
    use golem_rust_macro::ToolError;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use test_r::test;

    #[derive(ToolError)]
    enum GrepError {
        #[tool_error(kind = "usage-error", exit_code = 2)]
        BadPattern(String),
        #[tool_error(kind = "runtime-error", exit_code = 1)]
        Io(String),
    }

    /// Search files for a regex pattern.
    #[tool_definition(version = "1.2.3")]
    trait Grep {
        #[arg(case_sensitive = "global", short = 'i', kind = "flag")]
        #[arg(pattern = "positional", regex = r"^.+$")]
        #[arg(files = "tail", accepts_stdio = true)]
        fn grep(
            &self,
            case_sensitive: bool,
            pattern: String,
            files: Vec<String>,
        ) -> Result<Vec<String>, GrepError>;
    }

    struct GrepImpl;

    #[tool_implementation]
    impl Grep for GrepImpl {
        fn grep(
            &self,
            _case_sensitive: bool,
            _pattern: String,
            _files: Vec<String>,
        ) -> Result<Vec<String>, GrepError> {
            Ok(vec![])
        }
    }

    fn descriptor<T: Grep>() -> ExtendedToolType {
        <T as Grep>::__tool_descriptor()
    }

    #[test]
    fn grep_descriptor_builds() {
        let tool = descriptor::<GrepImpl>();
        assert_eq!(tool.version, "1.2.3");
        assert_eq!(tool.tool_name(), "grep");
        // Root command has a body (implicit-body method).
        let root = &tool.commands[0];
        assert_eq!(root.name, "grep");
        let body = root.body.as_ref().expect("root has a body");
        // `case_sensitive` is a global flag (declared global on the root).
        assert_eq!(root.globals.flags.len(), 1);
        assert_eq!(root.globals.flags[0].long, "case-sensitive");
        assert_eq!(root.globals.flags[0].short, Some('i'));
        // `pattern` is a fixed positional; `files` is the tail positional.
        assert_eq!(body.positionals.fixed.len(), 1);
        assert_eq!(body.positionals.fixed[0].name, "pattern");
        let tail = body.positionals.tail.as_ref().expect("tail positional");
        assert_eq!(tail.name, "files");
        assert!(tail.accepts_stdio);
        // Error cases are read from the `#[derive(ToolError)]` enum.
        assert_eq!(body.errors.len(), 2);
        assert_eq!(body.errors[0].name, "bad-pattern");
        assert_eq!(body.errors[0].exit_code, 2);
        // The whole thing converts to a valid wire tool.
        tool.try_to_tool().expect("grep tool is valid");
    }

    #[test]
    fn grep_error_cases() {
        let cases = GrepError::error_cases().expect("error cases build");
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].name, "bad-pattern");
        assert_eq!(cases[1].name, "io");
    }

    // --- subtree: git + remote ---------------------------------------------

    /// Manage remotes.
    #[tool_definition]
    trait Remote {
        /// Add a remote.
        fn add(&self, verbose: bool, name: String, url: String) -> Result<(), RemoteError>;
        /// Remove a remote.
        #[command(name = "rm")]
        fn remove(&self, name: String) -> Result<(), RemoteError>;
    }

    #[derive(ToolError)]
    enum RemoteError {
        #[tool_error(kind = "runtime-error", exit_code = 1)]
        Failed(String),
    }

    struct RemoteImpl;

    #[tool_implementation]
    impl Remote for RemoteImpl {
        fn add(&self, _verbose: bool, _name: String, _url: String) -> Result<(), RemoteError> {
            Ok(())
        }
        fn remove(&self, _name: String) -> Result<(), RemoteError> {
            Ok(())
        }
    }

    /// The stupid content tracker.
    #[tool_definition]
    trait Git {
        #[arg(config = "option", repeatable = "repeated")]
        #[command(aliases = ["ci"])]
        fn commit(
            &self,
            message: String,
            config: BTreeMap<String, String>,
        ) -> Result<(), CommitError>;

        // The subtree method declares `verbose` as a propagating global; the
        // child `Remote::add` repeats it as a body parameter, which must be
        // de-projected (suppressed) when grafted under this inherited global.
        #[command(subtree = Remote)]
        fn remote(&self, verbose: bool) -> RemoteSubtree;
    }

    // A placeholder return type for the subtree dispatcher method; never used.
    struct RemoteSubtree;

    #[derive(ToolError)]
    enum CommitError {
        #[tool_error(kind = "runtime-error", exit_code = 1)]
        Failed(String),
    }

    struct GitImpl;

    #[tool_implementation]
    impl Git for GitImpl {
        fn commit(
            &self,
            _message: String,
            _config: BTreeMap<String, String>,
        ) -> Result<(), CommitError> {
            Ok(())
        }
        fn remote(&self, _verbose: bool) -> RemoteSubtree {
            RemoteSubtree
        }
    }

    #[test]
    fn git_subtree_grafts_remote() {
        // Build with an explicit ctx so the subtree graft path is exercised.
        let tool = __golem_tool_descriptor_for_Git(&mut ToolBuildCtx::new())
            .expect("git descriptor builds");
        assert_eq!(tool.tool_name(), "git");
        let root = &tool.commands[0];
        // git has two subcommands: commit and remote.
        assert_eq!(root.subcommands.len(), 2);

        // The `commit` command carries a repeatable-map option for `config`.
        let commit = root
            .subcommands
            .iter()
            .map(|&i| &tool.commands[i as usize])
            .find(|c| c.name == "commit")
            .expect("commit command");
        assert_eq!(commit.aliases, vec!["ci".to_string()]);
        let commit_body = commit.body.as_ref().expect("commit body");
        let config_opt = commit_body
            .options
            .iter()
            .find(|o| o.long == "config")
            .expect("config option");
        assert!(matches!(
            config_opt.shape,
            ExtendedOptionShape::RepeatableMap(_)
        ));

        // The `remote` graft is a pure dispatcher (no body) with `add` and `rm`.
        let remote = root
            .subcommands
            .iter()
            .map(|&i| &tool.commands[i as usize])
            .find(|c| c.name == "remote")
            .expect("remote command");
        assert!(remote.body.is_none());
        // The subtree method's `verbose` parameter became a global flag on the
        // `remote` dispatcher, propagating to every descendant body.
        assert!(
            remote.globals.flags.iter().any(|f| f.long == "verbose"),
            "remote dispatcher carries the inherited `verbose` global flag"
        );
        let sub_names: Vec<&str> = remote
            .subcommands
            .iter()
            .map(|&i| tool.commands[i as usize].name.as_str())
            .collect();
        assert!(sub_names.contains(&"add"));
        assert!(sub_names.contains(&"rm"));

        // `Remote::add` repeats `verbose` in its Rust signature; once grafted
        // under the inherited global it must be de-projected from the body so
        // the canonical shape stays valid (no body-local / inherited-global
        // collision).
        let add = remote
            .subcommands
            .iter()
            .map(|&i| &tool.commands[i as usize])
            .find(|c| c.name == "add")
            .expect("add command");
        let add_body = add.body.as_ref().expect("add body");
        assert!(
            !add_body.flags.iter().any(|f| f.long == "verbose"),
            "inherited `verbose` global must be suppressed from the `add` body"
        );

        tool.try_to_tool().expect("git tool is valid");
    }

    // --- multi-level subtree: inherited-global suppression at depth ---------
    //
    // outer -> mid -> inner -> leaf, where each dispatcher level re-declares the
    // same propagating `verbose` global. Every child trait is synthesized
    // standalone, so the intermediate `inner` dispatcher carries its own
    // `verbose` global; once grafted under `mid` (which already supplies
    // `verbose`), that nested duplicate must be pruned or the validator rejects
    // the colliding inherited global.

    #[tool_definition]
    trait Inner {
        /// A leaf under inner.
        fn leaf(&self, verbose: bool, name: String) -> Result<(), RemoteError>;
    }

    // Placeholder return types for the subtree dispatcher methods; never used.
    struct InnerSubtree;
    struct MidSubtree;

    #[tool_definition]
    trait Mid {
        #[command(subtree = Inner)]
        fn inner(&self, verbose: bool) -> InnerSubtree;
    }

    #[tool_definition]
    trait Outer {
        #[command(subtree = Mid)]
        fn mid(&self, verbose: bool) -> MidSubtree;
    }

    #[test]
    fn multilevel_subtree_suppresses_inherited_globals_at_depth() {
        let tool = __golem_tool_descriptor_for_Outer(&mut ToolBuildCtx::new())
            .expect("outer descriptor builds");
        assert_eq!(tool.tool_name(), "outer");

        let find = |name: &str| {
            tool.commands
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("command `{name}` present"))
        };

        // `verbose` is supplied once at the `mid` dispatcher (the top subtree
        // method's global).
        let mid = find("mid");
        assert!(
            mid.globals.flags.iter().any(|f| f.long == "verbose"),
            "the `mid` dispatcher carries the `verbose` global"
        );
        // The nested `inner` dispatcher independently declared `verbose` when
        // synthesized standalone; grafted under `mid` it must be pruned.
        let inner = find("inner");
        assert!(
            !inner.globals.flags.iter().any(|f| f.long == "verbose"),
            "nested dispatcher must not redeclare the inherited `verbose` global"
        );
        // The `leaf` body repeated `verbose`; it must be suppressed too.
        let leaf = find("leaf");
        let leaf_body = leaf.body.as_ref().expect("leaf body");
        assert!(
            !leaf_body.flags.iter().any(|f| f.long == "verbose"),
            "inherited `verbose` global must be suppressed from the `leaf` body"
        );

        // Valid only if both the nested duplicate global and the body copy were
        // pruned.
        tool.try_to_tool()
            .expect("multi-level subtree tool is valid");
    }

    // --- subtree inheriting a global from the parent ROOT command -----------
    //
    // `verbose` is declared once on the `cli` root command. It is in effective
    // scope for the whole `mid` subtree even though the `mid` dispatcher method
    // contributes no globals of its own, so the nested `inner` dispatcher's
    // standalone `verbose` global (and any body copy) must still be suppressed.

    #[tool_definition]
    trait Cli {
        #[arg(verbose = "global", kind = "flag")]
        fn cli(&self, verbose: bool, target: String) -> Result<(), RemoteError>;

        #[command(subtree = Mid)]
        fn mid(&self) -> MidSubtree;
    }

    #[test]
    fn subtree_inherits_global_from_parent_root_command() {
        let tool = __golem_tool_descriptor_for_Cli(&mut ToolBuildCtx::new())
            .expect("cli descriptor builds");
        assert_eq!(tool.tool_name(), "cli");

        let find = |name: &str| {
            tool.commands
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("command `{name}` present"))
        };

        // `verbose` is declared once, on the `cli` root command.
        let root = find("cli");
        assert!(
            root.globals.flags.iter().any(|f| f.long == "verbose"),
            "the `cli` root command carries the `verbose` global"
        );

        // The nested `inner` dispatcher (from Mid) redeclared `verbose` when
        // synthesized standalone; under `cli`'s inherited root global it must be
        // pruned even though `cli`'s `mid` dispatcher method contributes no
        // globals of its own.
        let inner = find("inner");
        assert!(
            !inner.globals.flags.iter().any(|f| f.long == "verbose"),
            "nested dispatcher must not redeclare the root-inherited `verbose` global"
        );

        tool.try_to_tool()
            .expect("subtree inheriting a root global is valid");
    }

    #[tool_definition]
    trait TypeMismatchChild {
        fn leaf(&self, verbose: String, name: String) -> Result<(), RemoteError>;
    }

    struct TypeMismatchChildSubtree;

    #[tool_definition]
    trait TypeMismatchParent {
        #[command(subtree = TypeMismatchChild)]
        fn type_mismatch_child(&self, verbose: bool) -> TypeMismatchChildSubtree;
    }

    #[test]
    fn inherited_global_conflict_with_different_typed_child_parameter_is_rejected() {
        // The child `leaf` declares `verbose: String` while the parent subtree
        // method declares the propagating global `verbose: bool`. The shapes are
        // incompatible, so the composition is invalid: the descriptor build must
        // reject it rather than silently dropping or replacing either `verbose`.
        let err = __golem_tool_descriptor_for_TypeMismatchParent(&mut ToolBuildCtx::new())
            .expect_err("a child parameter colliding with an inherited global of a different shape must be rejected");
        assert!(
            matches!(
                err,
                ToolBuildError::InheritedGlobalConflict { ref name, ref inherited, ref command }
                    if name == "verbose" && inherited == "verbose" && command == "leaf"
            ),
            "expected an InheritedGlobalConflict for `verbose` on `leaf`, got {err:?}",
        );
    }

    struct Meters;

    impl QuantityUnit for Meters {
        fn type_id() -> golem_rust::schema::TypeId {
            golem_rust::schema::TypeId::new("golem.test.Meters")
        }

        fn base_unit() -> &'static str {
            "m"
        }
    }

    struct Seconds;

    impl QuantityUnit for Seconds {
        fn type_id() -> golem_rust::schema::TypeId {
            golem_rust::schema::TypeId::new("golem.test.Seconds")
        }

        fn base_unit() -> &'static str {
            "s"
        }
    }

    #[tool_definition]
    trait QuantityGlobalMismatch {
        #[arg(amount = "global")]
        fn quantity_global_mismatch(
            &self,
            amount: Quantity<Meters>,
            target: String,
        ) -> Result<(), RemoteError>;

        fn leaf(&self, amount: Quantity<Seconds>, name: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn leaf_redeclaring_quantity_root_global_with_different_unit_is_rejected() {
        let err = __golem_tool_descriptor_for_QuantityGlobalMismatch(&mut ToolBuildCtx::new())
            .expect_err(
                "a leaf re-declaring a quantity root global with a different unit spec must be rejected",
            );
        assert!(
            matches!(
                err,
                ToolBuildError::InheritedGlobalConflict { ref name, ref inherited, ref command }
                    if name == "amount" && inherited == "amount" && command == "leaf"
            ),
            "expected an InheritedGlobalConflict for quantity `amount` on `leaf`, got {err:?}",
        );
    }

    // A leaf command in the SAME trait as the root command re-declaring a root
    // global with an incompatible type is rejected (the root-global inheritance
    // path, not the subtree path).
    #[tool_definition]
    trait RootGlobalLeafMismatch {
        #[arg(verbose = "global", kind = "flag")]
        fn root_global_leaf_mismatch(
            &self,
            verbose: bool,
            target: String,
        ) -> Result<(), RemoteError>;
        fn leaf(&self, verbose: String, name: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn leaf_redeclaring_root_global_with_mismatched_type_is_rejected() {
        let err = __golem_tool_descriptor_for_RootGlobalLeafMismatch(&mut ToolBuildCtx::new())
            .expect_err(
                "a leaf re-declaring a root global with an incompatible type must be rejected",
            );
        assert!(
            matches!(
                err,
                ToolBuildError::InheritedGlobalConflict { ref name, ref inherited, ref command }
                    if name == "verbose" && inherited == "verbose" && command == "leaf"
            ),
            "expected an InheritedGlobalConflict for `verbose` on `leaf`, got {err:?}",
        );
    }

    // A leaf command in the same trait as the root re-declaring a root global
    // with a COMPATIBLE type has the body copy removed; the inherited global is
    // the single declaration.
    #[tool_definition]
    trait RootGlobalLeafMatch {
        #[arg(verbose = "global", kind = "flag")]
        fn root_global_leaf_match(&self, verbose: bool, target: String) -> Result<(), RemoteError>;
        fn leaf(&self, verbose: bool, name: String) -> Result<(), RemoteError>;
    }

    #[tool_definition]
    trait RootGlobalLeafMatchInferredBool {
        #[arg(verbose = "global")]
        fn root_global_leaf_match_inferred_bool(
            &self,
            verbose: bool,
            target: String,
        ) -> Result<(), RemoteError>;
        fn leaf(&self, verbose: bool, name: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn leaf_redeclaring_root_global_with_matching_type_is_suppressed() {
        let tool = __golem_tool_descriptor_for_RootGlobalLeafMatch(&mut ToolBuildCtx::new())
            .expect("matching root-global re-declaration builds");
        let leaf = tool
            .commands
            .iter()
            .find(|c| c.name == "leaf")
            .expect("leaf command");
        let body = leaf.body.as_ref().expect("leaf body");
        assert!(
            !body.flags.iter().any(|f| f.long == "verbose"),
            "the inherited `verbose` root global must be suppressed from the leaf body"
        );
        let root = &tool.commands[0];
        assert!(
            root.globals.flags.iter().any(|f| f.long == "verbose"),
            "the root command keeps the `verbose` global"
        );
        tool.try_to_tool()
            .expect("matching root-global re-declaration is valid");
    }

    #[test]
    fn explicit_global_bool_still_infers_global_flag() {
        let tool =
            __golem_tool_descriptor_for_RootGlobalLeafMatchInferredBool(&mut ToolBuildCtx::new())
                .expect(
                    "a bool global should project as a flag and de-project a matching leaf flag",
                );
        let root = &tool.commands[0];
        assert!(
            root.globals.flags.iter().any(|f| f.long == "verbose"),
            "a bool global should be exposed as a global flag"
        );
        assert!(
            root.globals.options.iter().all(|o| o.long != "verbose"),
            "a bool global should not be exposed as a value-taking global option"
        );
        let leaf = tool
            .commands
            .iter()
            .find(|c| c.name == "leaf")
            .expect("leaf command");
        let body = leaf.body.as_ref().expect("leaf body");
        assert!(
            !body.flags.iter().any(|f| f.long == "verbose"),
            "the matching inherited bool global must be suppressed from the leaf body"
        );
        tool.try_to_tool()
            .expect("matching inferred bool global re-declaration is valid");
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    struct RecursiveNode {
        next: Option<Box<RecursiveNode>>,
    }

    #[tool_definition]
    trait RootGlobalRecursiveLeafMatch {
        #[arg(node = "global")]
        fn root_global_recursive_leaf_match(
            &self,
            node: RecursiveNode,
            target: String,
        ) -> Result<(), RemoteError>;
        fn leaf(&self, node: RecursiveNode, name: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn leaf_redeclaring_recursive_root_global_with_matching_type_is_suppressed() {
        let tool =
            __golem_tool_descriptor_for_RootGlobalRecursiveLeafMatch(&mut ToolBuildCtx::new())
                .expect("the exact same recursive root-global re-declaration should be compatible");
        let leaf = tool
            .commands
            .iter()
            .find(|c| c.name == "leaf")
            .expect("leaf command");
        let body = leaf.body.as_ref().expect("leaf body");
        assert!(
            !body.positionals.fixed.iter().any(|p| p.name == "node"),
            "the inherited recursive `node` root global must be suppressed from the leaf body"
        );
        tool.try_to_tool()
            .expect("matching recursive root-global re-declaration is valid");
    }

    // A subtree dispatcher param repeating a root global with an incompatible
    // type is rejected (the subtree-method global path).
    #[tool_definition]
    trait DispatcherMismatchChild {
        fn leaf(&self, name: String) -> Result<(), RemoteError>;
    }

    struct DispatcherMismatchSubtree;

    #[tool_definition]
    trait DispatcherMismatchParent {
        #[arg(format = "global")]
        fn dispatcher_mismatch_parent(
            &self,
            format: String,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = DispatcherMismatchChild)]
        fn dispatcher_mismatch_child(&self, format: bool) -> DispatcherMismatchSubtree;
    }

    #[test]
    fn subtree_dispatcher_param_conflicting_with_root_global_is_rejected() {
        let err = __golem_tool_descriptor_for_DispatcherMismatchParent(&mut ToolBuildCtx::new())
            .expect_err(
                "a subtree dispatcher param conflicting with a root global must be rejected",
            );
        assert!(
            matches!(
                err,
                ToolBuildError::InheritedGlobalConflict { ref name, .. } if name == "format"
            ),
            "expected an InheritedGlobalConflict for `format`, got {err:?}",
        );
    }

    // An inherited re-declaration that is explicitly marked as a tail must not
    // steal the body's single tail slot from a genuine `Vec<T>` body tail: it is
    // lowered to a droppable repeatable-list option surrogate and removed by
    // normalization (when shape-compatible with the inherited global).
    #[tool_definition]
    trait InheritedExplicitTail {
        #[arg(items = "global")]
        fn inherited_explicit_tail(
            &self,
            items: Vec<String>,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(items = "tail")]
        fn collect(&self, items: Vec<String>, rest: Vec<String>) -> Result<(), RemoteError>;
    }

    #[test]
    fn inherited_explicit_tail_does_not_steal_genuine_tail_slot() {
        let tool = __golem_tool_descriptor_for_InheritedExplicitTail(&mut ToolBuildCtx::new())
            .expect("inherited explicit tail descriptor builds");
        let collect = tool
            .commands
            .iter()
            .find(|c| c.name == "collect")
            .expect("collect command");
        let body = collect.body.as_ref().expect("collect body");
        assert_eq!(
            body.positionals.tail.as_ref().map(|t| t.name.as_str()),
            Some("rest"),
            "the genuine `Vec<T>` body tail must keep the tail slot"
        );
        assert!(
            !body.options.iter().any(|o| o.long == "items")
                && body.positionals.tail.as_ref().map(|t| t.name.as_str()) != Some("items"),
            "the inherited `items` re-declaration (lowered to a surrogate option) must be removed"
        );
        assert!(
            tool.commands[0]
                .globals
                .options
                .iter()
                .any(|o| o.long == "items"),
            "the root keeps the `items` global"
        );
        tool.try_to_tool()
            .expect("inherited explicit tail tool is valid");
    }

    // The same surrogate path surfaces a real shape conflict instead of silently
    // dropping the parameter: an inherited `Vec<u32>` global re-declared as an
    // explicit tail of `Vec<String>` must be rejected.
    #[tool_definition]
    trait InheritedExplicitTailMismatch {
        #[arg(items = "global")]
        fn inherited_explicit_tail_mismatch(
            &self,
            items: Vec<u32>,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(items = "tail")]
        fn collect(&self, items: Vec<String>) -> Result<(), RemoteError>;
    }

    #[test]
    fn inherited_explicit_tail_with_mismatched_item_type_is_rejected() {
        let err =
            __golem_tool_descriptor_for_InheritedExplicitTailMismatch(&mut ToolBuildCtx::new())
                .expect_err(
                    "an inherited explicit tail of a mismatched item type must be rejected",
                );
        assert!(
            matches!(
                err,
                ToolBuildError::InheritedGlobalConflict { ref name, ref inherited, ref command }
                    if name == "items" && inherited == "items" && command == "collect"
            ),
            "expected an InheritedGlobalConflict for `items` on `collect`, got {err:?}",
        );
    }

    // A body option whose ALIAS collides with an inherited global of an
    // incompatible shape is rejected, and the error names the actually-colliding
    // surface token (`verbose`, the alias) plus the inherited global it hit.
    #[tool_definition]
    trait AliasConflict {
        #[arg(verbose = "global", kind = "flag")]
        fn alias_conflict(&self, verbose: bool, target: String) -> Result<(), RemoteError>;
        #[arg(local = "option", aliases = ["verbose"])]
        fn leaf(&self, local: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn body_option_alias_colliding_with_inherited_global_reports_alias() {
        let err = __golem_tool_descriptor_for_AliasConflict(&mut ToolBuildCtx::new())
            .expect_err("a body option alias colliding with an inherited global must be rejected");
        assert!(
            matches!(
                err,
                ToolBuildError::InheritedGlobalConflict { ref name, ref inherited, ref command }
                    if name == "verbose" && inherited == "verbose" && command == "leaf"
            ),
            "expected the conflict to name the colliding alias `verbose`, got {err:?}",
        );
    }

    // If one local declaration collides with more than one inherited global,
    // normalization must not stop after the first compatible collision and miss
    // a later incompatible alias collision. Here `format` is compatible with the
    // inherited string global, but the same option's alias `count` is
    // incompatible with the inherited u32 global.
    #[tool_definition]
    trait AliasMultiCollisionConflict {
        #[arg(format = "global")]
        #[arg(count = "global")]
        fn alias_multi_collision_conflict(
            &self,
            format: String,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(format = "option", aliases = ["count"])]
        fn leaf(&self, format: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn body_option_alias_conflict_after_compatible_collision_is_rejected() {
        let err = __golem_tool_descriptor_for_AliasMultiCollisionConflict(&mut ToolBuildCtx::new())
            .expect_err(
                "a local option alias colliding with an incompatible inherited global must be rejected even if its long name also collides compatibly",
            );
        assert!(
            matches!(
                err,
                ToolBuildError::InheritedGlobalConflict { ref name, ref inherited, ref command }
                    if name == "count" && inherited == "count" && command == "leaf"
            ),
            "expected an InheritedGlobalConflict for alias `count` on `leaf`, got {err:?}",
        );
    }

    #[tool_definition]
    trait AliasMultiCompatibleCollisionConflict {
        #[arg(format = "global")]
        #[arg(profile = "global")]
        fn alias_multi_compatible_collision_conflict(
            &self,
            format: String,
            profile: String,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(format = "option", aliases = ["profile"])]
        fn leaf(&self, format: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn body_option_alias_colliding_with_two_compatible_inherited_globals_is_rejected() {
        let err = __golem_tool_descriptor_for_AliasMultiCompatibleCollisionConflict(
            &mut ToolBuildCtx::new(),
        )
        .expect_err(
            "one local parameter must not be de-projected onto two distinct inherited globals",
        );
        assert!(
            matches!(err, ToolBuildError::InheritedGlobalConflict { ref command, .. } if command == "leaf"),
            "expected an inherited-global conflict for the ambiguous `leaf` parameter, got {err:?}",
        );
    }

    // Cross-subtree tail-inference boundary (oracle finding 2): a subtree child
    // is synthesized standalone, before it knows which of its parameters the
    // parent hoists into a propagating global, so a child subcommand cannot rely
    // on *inferred* tail promotion that only becomes correct post-composition.
    // The supported pattern is to annotate the child explicitly. This proves the
    // explicit pattern survives composition: `items` stays the tail and the
    // hoisted `format` option is reconciled away under the inherited global.
    #[tool_definition]
    trait Fetcher {
        #[arg(items = "tail")]
        #[arg(format = "option")]
        fn collect(&self, items: Vec<String>, format: String) -> Result<(), RemoteError>;
    }

    struct FetcherSubtree;

    #[tool_definition]
    trait Downloader {
        #[command(subtree = Fetcher)]
        fn fetcher(&self, format: String) -> FetcherSubtree;
    }

    #[test]
    fn explicit_child_tail_survives_subtree_global_hoisting() {
        let tool = __golem_tool_descriptor_for_Downloader(&mut ToolBuildCtx::new())
            .expect("downloader subtree descriptor builds");
        let collect = tool
            .commands
            .iter()
            .find(|c| c.name == "collect")
            .expect("collect subcommand");
        let body = collect.body.as_ref().expect("collect body");
        assert_eq!(
            body.positionals.tail.as_ref().map(|t| t.name.as_str()),
            Some("items"),
            "the explicit child tail must survive composition under the hoisted global"
        );
        assert!(
            !body.options.iter().any(|o| o.long == "format"),
            "the child `format` option must be reconciled away under the inherited `format` global"
        );
        tool.try_to_tool().expect("downloader tool is valid");
    }

    // Tail inference must survive a trailing parameter that repeats an inherited
    // root global: the inherited duplicate is removed by normalization, and the
    // preceding `Vec<T>` remains a tail positional.
    #[tool_definition]
    trait TailWithInheritedTrailingGlobal {
        #[arg(format = "global")]
        fn tail_with_inherited_trailing_global(
            &self,
            format: String,
            target: String,
        ) -> Result<(), RemoteError>;
        fn collect(&self, items: Vec<String>, format: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn tail_inference_survives_trailing_inherited_global() {
        let tool =
            __golem_tool_descriptor_for_TailWithInheritedTrailingGlobal(&mut ToolBuildCtx::new())
                .expect("descriptor builds");
        let collect = tool
            .commands
            .iter()
            .find(|c| c.name == "collect")
            .expect("collect command");
        let body = collect.body.as_ref().expect("collect body");
        assert_eq!(
            body.positionals.tail.as_ref().map(|t| t.name.as_str()),
            Some("items"),
            "a trailing parameter repeating an inherited global must not prevent tail inference"
        );
        assert!(
            !body.positionals.fixed.iter().any(|p| p.name == "format")
                && !body.options.iter().any(|o| o.long == "format"),
            "the inherited `format` re-declaration must be removed from the collect body"
        );
        tool.try_to_tool().expect("tool is valid");
    }

    #[tool_definition]
    trait TailBeforeGlobal {
        #[arg(format = "global")]
        fn collect(&self, items: Vec<String>, format: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn trailing_global_argument_does_not_prevent_vec_tail_inference() {
        let tool = __golem_tool_descriptor_for_TailBeforeGlobal(&mut ToolBuildCtx::new())
            .expect("tail-before-global descriptor builds");
        let collect = tool
            .commands
            .iter()
            .find(|c| c.name == "collect")
            .expect("collect command");
        let body = collect.body.as_ref().expect("collect body");

        assert!(
            collect.globals.options.iter().any(|o| o.long == "format"),
            "the trailing scalar global should project to a global option"
        );
        assert!(
            body.options.iter().all(|o| o.long != "items"),
            "the final non-global Vec<T> parameter should not be projected as a repeatable option"
        );
        assert_eq!(
            body.positionals.tail.as_ref().map(|t| t.name.as_str()),
            Some("items"),
            "a global parameter after the Vec<T> should not prevent tail positional inference"
        );
    }

    #[tool_definition]
    trait TailInference {
        fn collect(&self, prefix: String, items: Vec<String>) -> Result<(), RemoteError>;
    }

    struct TailInferenceImpl;

    #[tool_implementation]
    impl TailInference for TailInferenceImpl {
        fn collect(&self, _prefix: String, _items: Vec<String>) -> Result<(), RemoteError> {
            Ok(())
        }
    }

    #[test]
    fn vec_parameter_in_tail_position_is_tail_positional_without_attribute() {
        let tool = __golem_tool_descriptor_for_TailInference(&mut ToolBuildCtx::new())
            .expect("tail inference descriptor builds");
        let collect = tool
            .commands
            .iter()
            .find(|c| c.name == "collect")
            .expect("collect command");
        let body = collect.body.as_ref().expect("collect body");

        assert!(
            body.options.iter().all(|o| o.long != "items"),
            "a trailing Vec<T> parameter should not be projected as a repeatable option"
        );
        assert_eq!(
            body.positionals.tail.as_ref().map(|t| t.name.as_str()),
            Some("items"),
            "a trailing Vec<T> parameter should be projected as the tail positional"
        );
    }

    #[tool_definition]
    trait EmptyMapDefault {
        #[arg(env = "option", default = [])]
        fn set_env(&self, env: BTreeMap<String, String>) -> Result<(), RemoteError>;
    }

    #[test]
    fn empty_array_default_for_map_option_builds_empty_map() {
        let tool = __golem_tool_descriptor_for_EmptyMapDefault(&mut ToolBuildCtx::new())
            .expect("an empty array default on a map option should build an empty map default");
        let set_env = tool
            .commands
            .iter()
            .find(|c| c.name == "set-env")
            .expect("set-env command");
        let body = set_env.body.as_ref().expect("set-env body");
        let env = body
            .options
            .iter()
            .find(|o| o.long == "env")
            .expect("env option");

        assert!(
            env.default.is_some(),
            "map option default should be present"
        );
        tool.try_to_tool()
            .expect("tool with empty map default should be valid");
    }

    #[test]
    fn non_bool_flag_parameter_is_rejected() {
        let output = cargo_check_tool_crate(
            "non-bool-flag",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(level = "flag")]
    fn run(&self, level: u32) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _level: u32) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a bool flag projected from a u32 parameter should be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn count_flag_parameter_must_be_u32() {
        let output = cargo_check_tool_crate(
            "count-flag-u64",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(verbose = "flag", kind = "count-flag")]
    fn run(&self, verbose: u64) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _verbose: u64) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a count flag is exposed as a u32 input field, so a u64 implementation parameter should be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn count_flag_parameter_must_not_be_optional_u32() {
        let output = cargo_check_tool_crate(
            "count-flag-option-u32",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(verbose = "flag", kind = "count-flag")]
    fn run(&self, verbose: Option<u32>) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _verbose: Option<u32>) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a count flag is exposed as a u32 input field, so an Option<u32> implementation parameter should be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn explicit_tail_parameter_before_fixed_positional_is_rejected() {
        let output = cargo_check_tool_crate(
            "tail-before-fixed",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(items = "tail")]
    fn run(&self, items: Vec<String>, suffix: String) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _items: Vec<String>, _suffix: String) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "an explicit tail positional before a later fixed positional should be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn optional_vec_tail_is_rejected_instead_of_dropping_optionality() {
        let output = cargo_check_tool_crate(
            "optional-vec-tail",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(items = "tail")]
    fn run(&self, items: Option<Vec<String>>) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _items: Option<Vec<String>>) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "an Option<Vec<T>> tail positional cannot be represented in ExtendedTailPositional and must be rejected instead of silently dropping optionality, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn scalar_global_argument_projects_to_global_option() {
        let output = cargo_check_tool_crate(
            "scalar-global-option",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(format = "global")]
    fn bad_tool(&self, format: String) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn bad_tool(&self, _format: String) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "an explicitly global scalar argument should project to a global option, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn optional_fixed_positional_before_required_fixed_positional_is_rejected() {
        let output = cargo_check_tool_crate(
            "optional-before-required-positional",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    fn run(&self, maybe_prefix: Option<String>, required_name: String) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(
        &self,
        _maybe_prefix: Option<String>,
        _required_name: String,
    ) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "an optional fixed positional before a required fixed positional should be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn text_refinement_on_non_text_parameter_is_rejected() {
        let output = cargo_check_tool_crate(
            "text-refinement-on-non-text",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(count = "positional", regex = "^[0-9]+$")]
    fn run(&self, count: u32) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _count: u32) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a text refinement on a non-text Rust parameter should be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn tail_min_max_are_occurrence_bounds_not_item_numeric_bounds() {
        let output = cargo_check_tool_crate(
            "tail-min-max-string-items",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait GoodTool {
    #[arg(items = "tail", min = 1, max = 3)]
    fn run(&self, items: Vec<String>) -> Result<(), BadError>;
}

struct GoodToolImpl;

#[tool_implementation]
impl GoodTool for GoodToolImpl {
    fn run(&self, _items: Vec<String>) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "tail min/max should constrain occurrence count only, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[tool_definition]
    trait NonFiniteNumericBound {
        #[arg(value = "option", min = f64::NAN)]
        fn run(&self, value: f64) -> Result<(), RemoteError>;
    }

    #[test]
    fn non_finite_numeric_bound_returns_tool_build_error_instead_of_panicking() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            __golem_tool_descriptor_for_NonFiniteNumericBound(&mut ToolBuildCtx::new())
        }));

        assert!(
            result.is_ok(),
            "descriptor build should return a ToolBuildError for a non-finite numeric bound, not panic"
        );
        assert!(
            result.expect("panic checked above").is_err(),
            "a non-finite numeric bound should be rejected"
        );
    }

    #[test]
    fn usize_option_numeric_bounds_compile() {
        let output = cargo_check_tool_crate(
            "usize-option-numeric-bounds",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait GoodTool {
    #[arg(count = "option", min = 1, max = 3)]
    fn run(&self, count: usize) -> Result<(), BadError>;
}

struct GoodToolImpl;

#[tool_implementation]
impl GoodTool for GoodToolImpl {
    fn run(&self, _count: usize) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "usize is a supported schema type, so numeric bounds on a usize option should compile\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[tool_definition]
    trait ValueIsRefinedLocalOption {
        #[arg(format = "option", regex = "^(json|yaml)$")]
        #[constraint(requires_all = value_is("format", "json"))]
        fn run(&self, format: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn value_is_constraint_uses_refined_local_option_schema() {
        let tool = __golem_tool_descriptor_for_ValueIsRefinedLocalOption(&mut ToolBuildCtx::new())
            .expect("descriptor builds");

        tool.try_to_tool().expect(
            "value_is on a refined option should validate against the refined option schema",
        );
    }

    type TagsAlias = Vec<String>;

    #[tool_definition]
    trait ValueIsListAliasOption {
        #[arg(tags = "option")]
        #[constraint(requires_all = value_is("tags", "prod"))]
        fn run(&self, tags: TagsAlias) -> Result<(), RemoteError>;
    }

    #[test]
    fn value_is_item_literal_works_for_list_alias_option() {
        let tool = __golem_tool_descriptor_for_ValueIsListAliasOption(&mut ToolBuildCtx::new())
            .expect("a value_is item literal should build for a list-shaped option alias");

        tool.try_to_tool()
            .expect("a value_is item literal should validate for a list-shaped option alias");
    }

    // A subtree child constraint can `value_is` a global supplied only by the
    // parent subtree method. The standalone child cannot type the literal, so it
    // is deferred and resolved when the parent composes the child.
    #[tool_definition]
    trait DeferredValueIsChild {
        #[constraint(requires_all = value_is("format", "json"))]
        fn leaf(&self, name: String) -> Result<(), RemoteError>;
    }

    struct DeferredValueIsSubtree;

    #[tool_definition]
    trait DeferredValueIsParent {
        #[command(subtree = DeferredValueIsChild)]
        fn deferred_value_is_child(&self, format: String) -> DeferredValueIsSubtree;
    }

    #[test]
    fn deferred_value_is_resolves_when_parent_composes_child() {
        let tool = __golem_tool_descriptor_for_DeferredValueIsParent(&mut ToolBuildCtx::new())
            .expect("parent composition should build the descriptor");
        // The deferred child `value_is("format", ...)` must resolve against the
        // parent-supplied `format` global and the composed tool must build.
        tool.try_to_tool()
            .expect("the composed tool with a resolved deferred value_is should build");
    }

    #[tool_definition]
    trait DeferredValueIsListAliasChild {
        #[constraint(requires_all = value_is("tags", "prod"))]
        fn leaf(&self, name: String) -> Result<(), RemoteError>;
    }

    struct DeferredValueIsListAliasSubtree;

    #[tool_definition]
    trait DeferredValueIsListAliasParent {
        #[arg(tags = "option")]
        #[command(subtree = DeferredValueIsListAliasChild)]
        fn deferred_value_is_list_alias_child(
            &self,
            tags: TagsAlias,
        ) -> DeferredValueIsListAliasSubtree;
    }

    #[test]
    fn deferred_value_is_item_literal_works_for_parent_list_alias_global() {
        let tool = __golem_tool_descriptor_for_DeferredValueIsListAliasParent(
            &mut ToolBuildCtx::new(),
        )
        .expect(
            "a deferred value_is item literal should resolve for a list-shaped parent global alias",
        );

        tool.try_to_tool().expect(
            "a resolved deferred item literal should validate for a list-shaped parent global alias",
        );
    }

    #[test]
    fn standalone_subtree_child_with_parent_only_value_is_fails_to_build() {
        // Built without the parent, the child's `value_is("format", ...)` names a
        // global no ancestor supplies, so `format` is not in the child's
        // constraint scope at all. The literal stays deferred and the build fails
        // as an unresolved constraint reference (the same error a `present`
        // reference to an unknown name would give), not a silent accept.
        let tool = __golem_tool_descriptor_for_DeferredValueIsChild(&mut ToolBuildCtx::new())
            .expect("standalone child descriptor builds with the literal still deferred");
        let err = tool.try_to_tool().unwrap_err();
        assert!(
            matches!(err, ToolBuildError::UnresolvedConstraintRef(ref name) if name == "format"),
            "expected an unresolved-constraint-ref error for `format`, got {err:?}",
        );
    }

    // A subtree child re-declares an argument the parent supplies as a global,
    // and its `value_is` literal is invalid against the *child-local*
    // restriction but valid against the *parent global's wider* restriction.
    // Composition removes the compatible child-local re-declaration (shape match,
    // restrictions ignored), so the constraint must resolve against the parent
    // global. A nested child therefore must defer resolution until the parent
    // composes it: normalizing the child standalone would reject a constraint
    // that is valid in the composed tool.
    #[tool_definition]
    trait RestrictionWidenChild {
        #[arg(count = "option", min = 0, max = 5)]
        #[constraint(requires_all = value_is("count", 15))]
        fn leaf(&self, count: u32) -> Result<(), RemoteError>;
    }

    struct RestrictionWidenSubtree;

    #[tool_definition]
    trait RestrictionWidenParent {
        #[arg(count = "option", min = 10, max = 20)]
        #[command(subtree = RestrictionWidenChild)]
        fn restriction_widen_child(&self, count: u32) -> RestrictionWidenSubtree;
    }

    #[test]
    fn nested_child_value_is_resolves_against_widened_parent_global_restriction() {
        let tool = __golem_tool_descriptor_for_RestrictionWidenParent(&mut ToolBuildCtx::new())
            .expect(
                "parent composition should build: the child `value_is(\"count\", 15)` resolves \
                 against the parent global's 10..=20 restriction, which accepts 15",
            );
        tool.try_to_tool().expect(
            "the composed tool must build: 15 is valid against the widened parent global restriction",
        );
    }

    #[tool_definition]
    trait TextRestrictionGlobalRedeclaration {
        #[arg(format = "global", regex = "^(json|yaml)$")]
        fn text_restriction_global_redeclaration(
            &self,
            format: String,
            target: String,
        ) -> Result<(), RemoteError>;

        fn leaf(&self, format: String, name: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn leaf_redeclaring_text_refined_string_global_is_suppressed() {
        let tool =
            __golem_tool_descriptor_for_TextRestrictionGlobalRedeclaration(&mut ToolBuildCtx::new())
                .expect(
                    "a leaf re-declaring the same String argument should be compatible with an inherited text-refined global; regex is a refinable restriction",
                );
        let leaf = tool
            .commands
            .iter()
            .find(|c| c.name == "leaf")
            .expect("leaf command");
        let body = leaf.body.as_ref().expect("leaf body");
        assert!(
            !body.positionals.fixed.iter().any(|p| p.name == "format"),
            "the inherited text-refined `format` global must be suppressed from the leaf body"
        );
        tool.try_to_tool()
            .expect("matching String redeclaration under a text refinement is valid");
    }

    #[test]
    fn standalone_child_value_is_violating_local_restriction_is_rejected() {
        // Built without the parent, the child's own `count` option restricts to
        // 0..=5, so `value_is("count", 15)` is contradictory and must be rejected
        // — the same constraint that the parent later widens to validity.
        match __golem_tool_descriptor_for_RestrictionWidenChild(&mut ToolBuildCtx::new()) {
            Err(ToolBuildError::ValueIsTypeMismatch(name)) if name == "count" => {}
            Err(other) => panic!("expected ValueIsTypeMismatch for `count`, got {other:?}"),
            Ok(tool) => panic!(
                "standalone child accepted a value_is literal that violates its local restriction; \
                 try_to_tool returned {:?}",
                tool.try_to_tool()
            ),
        }
    }

    #[test]
    fn value_is_constraint_can_reference_inherited_global_option() {
        let output = cargo_check_tool_crate(
            "value-is-inherited-global-option",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait GoodTool {
    #[arg(format = "global")]
    fn good_tool(&self, format: String, target: String) -> Result<(), BadError>;

    #[constraint(requires_all = value_is("format", "json"))]
    fn leaf(&self, name: String) -> Result<(), BadError>;
}

struct GoodToolImpl;

#[tool_implementation]
impl GoodTool for GoodToolImpl {
    fn good_tool(&self, _format: String, _target: String) -> Result<(), BadError> {
        Ok(())
    }

    fn leaf(&self, _name: String) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "a value_is constraint should be able to reference an inherited global option, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn value_is_constraint_can_reference_parent_subtree_global_option() {
        let output = cargo_check_tool_crate(
            "value-is-parent-subtree-global-option",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait ChildTool {
    #[constraint(requires_all = value_is("format", "json"))]
    fn leaf(&self, name: String) -> Result<(), BadError>;
}

struct ChildSubtree;

#[tool_definition]
trait ParentTool {
    #[command(subtree = ChildTool)]
    fn child_tool(&self, format: String) -> ChildSubtree;
}

struct ChildImpl;

#[tool_implementation]
impl ChildTool for ChildImpl {
    fn leaf(&self, _name: String) -> Result<(), BadError> {
        Ok(())
    }
}

struct ParentImpl;

#[tool_implementation]
impl ParentTool for ParentImpl {
    fn child_tool(&self, _format: String) -> ChildSubtree {
        ChildSubtree
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "a child subtree constraint should be able to value_is a global supplied by the parent subtree method, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn subtree_dispatcher_param_explicit_positional_is_rejected() {
        let output = cargo_check_tool_crate(
            "subtree-dispatcher-positional-param",
            r#"
use golem_rust::{tool_definition, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait ChildTool {
    fn leaf(&self, name: String) -> Result<(), BadError>;
}

struct ChildToolSubtree;

#[tool_definition]
trait ParentTool {
    #[arg(format = "positional")]
    #[command(subtree = ChildTool)]
    fn child_tool(&self, format: String) -> ChildToolSubtree;
}
"#,
        );

        assert!(
            !output.status.success(),
            "a subtree dispatcher parameter explicitly placed as positional cannot be represented on a pure dispatcher and should be rejected instead of silently becoming a global option\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn path_refinement_on_non_path_parameter_is_rejected() {
        let output = cargo_check_tool_crate(
            "path-refinement-on-non-path",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(count = "option", kind = "file")]
    fn run(&self, count: u32) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _count: u32) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a path refinement on a non-path Rust parameter should be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn numeric_unit_on_non_numeric_parameter_is_rejected() {
        let output = cargo_check_tool_crate(
            "numeric-unit-on-non-numeric",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(name = "option", unit = "ms")]
    fn run(&self, name: String) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _name: String) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a numeric unit on a non-numeric Rust parameter should be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[tool_definition]
    trait TailNumericUnit {
        #[arg(values = "tail", unit = "ms")]
        fn run(&self, values: Vec<u32>) -> Result<(), RemoteError>;
    }

    #[test]
    fn tail_numeric_unit_on_numeric_items_is_preserved() {
        let tool = __golem_tool_descriptor_for_TailNumericUnit(&mut ToolBuildCtx::new())
            .expect("numeric tail descriptor builds");
        let run = tool
            .commands
            .iter()
            .find(|c| c.name == "run")
            .expect("run command");
        let tail = run
            .body
            .as_ref()
            .and_then(|body| body.positionals.tail.as_ref())
            .expect("tail positional");

        assert_eq!(
            tail.item_type
                .root
                .numeric_restrictions()
                .and_then(|restrictions| restrictions.unit.as_deref()),
            Some("ms"),
            "numeric refinements on a tail positional's item type must not be silently dropped"
        );
    }

    #[tool_definition]
    trait TailItemBoundsWithOccurrenceMax {
        #[arg(values = "tail", bounds = (1, 3), max = 10)]
        fn run(&self, values: Vec<u32>) -> Result<(), RemoteError>;
    }

    #[test]
    fn tail_item_bounds_coexist_with_occurrence_max() {
        let tool =
            __golem_tool_descriptor_for_TailItemBoundsWithOccurrenceMax(&mut ToolBuildCtx::new())
                .expect("tail descriptor builds");
        let run = tool
            .commands
            .iter()
            .find(|c| c.name == "run")
            .expect("run command");
        let tail = run
            .body
            .as_ref()
            .and_then(|body| body.positionals.tail.as_ref())
            .expect("tail positional");

        // `max = 10` bounds the occurrence count, not the item value.
        assert_eq!(tail.max, Some(10), "occurrence max should be preserved");

        // `bounds = (1, 3)` refines the item's numeric schema.
        let restrictions = tail
            .item_type
            .root
            .numeric_restrictions()
            .expect("item carries numeric restrictions");
        assert_eq!(
            restrictions.min,
            Some(golem_rust::schema::schema_type::NumericBound::Unsigned(1)),
            "item lower bound should come from `bounds`"
        );
        assert_eq!(
            restrictions.max,
            Some(golem_rust::schema::schema_type::NumericBound::Unsigned(3)),
            "item upper bound should come from `bounds`"
        );
    }

    #[tool_definition]
    trait TailOccurrenceMinGreaterThanMax {
        #[arg(values = "tail", min = 5, max = 3)]
        fn run(&self, values: Vec<String>) -> Result<(), RemoteError>;
    }

    #[test]
    fn tail_occurrence_min_greater_than_max_is_rejected() {
        let tool =
            __golem_tool_descriptor_for_TailOccurrenceMinGreaterThanMax(&mut ToolBuildCtx::new())
                .expect("descriptor should build far enough to validate tail occurrence bounds");

        assert!(
            tool.try_to_tool().is_err(),
            "a tail positional with occurrence min greater than max is impossible and should be rejected"
        );
    }

    #[test]
    fn tail_numeric_bounds_on_non_numeric_items_are_rejected() {
        let output = cargo_check_tool_crate(
            "tail-numeric-bounds-on-non-numeric-items",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(values = "tail", bounds = (1, 3))]
    fn run(&self, values: Vec<String>) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _values: Vec<String>) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "numeric bounds on a non-numeric tail item should be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn text_refinement_on_bool_flag_is_rejected() {
        let output = cargo_check_tool_crate(
            "text-refinement-on-bool-flag",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(force = "flag", regex = "yes|no")]
    fn run(&self, force: bool) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _force: bool) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a text refinement on a bool flag should be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn option_with_count_flag_kind_is_rejected_instead_of_reclassified_as_flag() {
        let output = cargo_check_tool_crate(
            "option-with-count-flag-kind",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(level = "option", kind = "count-flag")]
    fn run(&self, level: u32) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _level: u32) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "an arg explicitly placed as an option must not be silently reclassified as a count flag by `kind = \"count-flag\"`, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn tail_default_is_rejected_instead_of_silently_dropped() {
        let output = cargo_check_tool_crate(
            "tail-default-is-rejected",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(items = "tail", default = ["a", "b"])]
    fn run(&self, items: Vec<String>) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _items: Vec<String>) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a default on a tail positional cannot be represented in ExtendedTailPositional and must be rejected instead of silently dropped, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn parenthesized_bool_flag_default_is_accepted_like_other_metadata_literals() {
        let output = cargo_check_tool_crate(
            "parenthesized-bool-flag-default",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait GoodTool {
    #[arg(force = "flag", default = (true))]
    fn run(&self, force: bool) -> Result<(), BadError>;
}

struct GoodToolImpl;

#[tool_implementation]
impl GoodTool for GoodToolImpl {
    fn run(&self, _force: bool) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "parenthesized bool literals are accepted by the metadata-literal grammar and should work as flag defaults, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn option_accepts_stdio_is_rejected_instead_of_silently_dropped() {
        let output = cargo_check_tool_crate(
            "option-accepts-stdio",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(input = "option", accepts_stdio = true)]
    fn run(&self, input: String) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _input: String) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "accepts_stdio has no field on options and should be rejected instead of silently dropped, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn value_name_on_flag_is_rejected_instead_of_silently_dropped() {
        let output = cargo_check_tool_crate(
            "value-name-on-flag",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(force = "flag", value_name = "FORCE")]
    fn run(&self, force: bool) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _force: bool) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a flag has no value_name field and it should be rejected instead of silently dropped, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn result_attr_on_unit_success_is_rejected_instead_of_silently_dropped() {
        let output = cargo_check_tool_crate(
            "result-attr-on-unit-success",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[result(formatters = ["json"], default = "json")]
    fn run(&self) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "#[result] on a unit-success method has no wire result slot and should be rejected instead of silently dropped, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn explicit_arg_attr_on_stdin_stream_is_rejected_instead_of_silently_dropped() {
        let output = cargo_check_tool_crate(
            "stream-arg-attr",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};
use golem_rust::wasip2::io::streams::InputStream;

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(input = "option")]
    fn run(&self, input: InputStream) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _input: InputStream) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a stdin/stdout stream is projected only from its type and any `#[arg]` field would be silently dropped, so it must be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn delim_without_delimited_repeatable_mode_is_rejected() {
        let output = cargo_check_tool_crate(
            "delim-without-mode",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum BadError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait BadTool {
    #[arg(tags = "option", delim = ',')]
    fn run(&self, tags: Vec<String>) -> Result<(), BadError>;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _tags: Vec<String>) -> Result<(), BadError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "a `delim` with the default `repeatable = \"repeated\"` mode would be silently dropped and must be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[tool_definition]
    trait ValueIsFlag {
        #[arg(force = "flag")]
        #[constraint(requires_all = value_is("force", true))]
        fn run(&self, force: bool) -> Result<(), RemoteError>;
    }

    #[test]
    fn value_is_against_flag_is_rejected_by_descriptor_build() {
        let err = __golem_tool_descriptor_for_ValueIsFlag(&mut ToolBuildCtx::new())
            .expect_err("value_is against a flag should be rejected during descriptor build");

        assert!(
            matches!(err, ToolBuildError::ValueIsTypeMismatch(ref name) if name == "force"),
            "expected ValueIsTypeMismatch for flag `force`, got {err:?}",
        );
    }

    #[tool_definition]
    trait ValueIsWrongLiteralType {
        #[arg(count = "option")]
        #[constraint(requires_all = value_is("count", "not-a-number"))]
        fn run(&self, count: u32) -> Result<(), RemoteError>;
    }

    #[test]
    fn value_is_literal_type_mismatch_reports_value_is_not_default() {
        let err = __golem_tool_descriptor_for_ValueIsWrongLiteralType(&mut ToolBuildCtx::new())
            .expect_err("bad value_is literal should be rejected during descriptor build");

        assert!(
            matches!(err, ToolBuildError::ValueIsTypeMismatch(ref name) if name == "count"),
            "expected ValueIsTypeMismatch for `count`, got {err:?}",
        );
    }

    #[tool_definition]
    trait ValueIsMapPositional {
        #[arg(config = "positional")]
        #[constraint(requires_all = value_is("config", 1))]
        fn run(&self, config: BTreeMap<String, u32>) -> Result<(), RemoteError>;
    }

    #[test]
    fn value_is_map_positional_uses_map_value_type_consistently() {
        let tool = __golem_tool_descriptor_for_ValueIsMapPositional(&mut ToolBuildCtx::new())
            .expect(
                "descriptor build resolves the map value_is literal against the map value type",
            );

        tool.try_to_tool().expect(
            "runtime validation should use the same map value comparand as descriptor build",
        );
    }

    #[derive(Clone)]
    struct FixedPair;

    impl golem_rust::schema::IntoSchema for FixedPair {
        fn type_id() -> golem_rust::schema::TypeId {
            golem_rust::schema::TypeId::new("tool_test.FixedPair")
        }

        fn register_in(
            _builder: &mut golem_rust::schema::SchemaBuilder,
        ) -> golem_rust::schema::SchemaType {
            golem_rust::schema::SchemaType::fixed_list(golem_rust::schema::SchemaType::u32(), 2)
        }

        fn to_value(&self) -> golem_rust::schema::SchemaValue {
            golem_rust::schema::SchemaValue::FixedList {
                elements: vec![
                    golem_rust::schema::SchemaValue::U32(1),
                    golem_rust::schema::SchemaValue::U32(2),
                ],
            }
        }
    }

    impl golem_rust::schema::FromSchema for FixedPair {
        fn from_value(
            value: &golem_rust::schema::SchemaValue,
        ) -> Result<Self, golem_rust::schema::FromSchemaError> {
            match value {
                golem_rust::schema::SchemaValue::FixedList { elements }
                | golem_rust::schema::SchemaValue::List { elements }
                    if elements.len() == 2 =>
                {
                    Ok(FixedPair)
                }
                _ => Err(golem_rust::schema::FromSchemaError::custom(
                    "expected a two-element fixed list",
                )),
            }
        }
    }

    #[tool_definition]
    trait ValueIsFixedListWholeValue {
        #[arg(pair = "positional")]
        #[constraint(requires_all = value_is("pair", [1, 2]))]
        fn run(&self, pair: FixedPair) -> Result<(), RemoteError>;
    }

    #[test]
    fn value_is_accepts_whole_fixed_list_literal() {
        let tool = __golem_tool_descriptor_for_ValueIsFixedListWholeValue(&mut ToolBuildCtx::new())
            .expect("a value_is literal compatible with the whole fixed-list type should build");

        tool.try_to_tool()
            .expect("runtime validation should also accept the whole fixed-list value");
    }

    #[tool_definition]
    trait ValueIsMapPositionalWithListValue {
        #[arg(config = "positional")]
        #[constraint(requires_all = value_is("config", 1))]
        fn run(&self, config: BTreeMap<String, Vec<u32>>) -> Result<(), RemoteError>;
    }

    #[test]
    fn value_is_map_positional_does_not_accept_nested_list_element_literal() {
        let result =
            __golem_tool_descriptor_for_ValueIsMapPositionalWithListValue(&mut ToolBuildCtx::new());

        match result {
            Err(ToolBuildError::ValueIsTypeMismatch(name)) if name == "config" => {}
            Err(other) => panic!(
                "expected ValueIsTypeMismatch for nested map-value element literal, got {other:?}"
            ),
            Ok(tool) => {
                let validation = tool.try_to_tool();
                panic!(
                    "descriptor accepted a value_is literal nested inside a list-valued map entry; runtime validation returned {validation:?}"
                );
            }
        }
    }

    #[tool_definition]
    trait ValueIsNestedListOption {
        #[arg(items = "option")]
        #[constraint(requires_all = value_is("items", 1))]
        fn run(&self, items: Vec<Vec<u32>>) -> Result<(), RemoteError>;
    }

    #[tool_definition]
    trait ValueIsMapOptionWithListValue {
        #[arg(config = "option")]
        #[constraint(requires_all = value_is("config", 1))]
        fn run(&self, config: BTreeMap<String, Vec<u32>>) -> Result<(), RemoteError>;
    }

    #[tool_definition]
    trait ValueIsNestedListTail {
        #[arg(items = "tail")]
        #[constraint(requires_all = value_is("items", 1))]
        fn run(&self, items: Vec<Vec<u32>>) -> Result<(), RemoteError>;
    }

    #[tool_definition]
    trait DeferredNestedListValueIsChild {
        #[constraint(requires_all = value_is("items", 1))]
        fn leaf(&self, name: String) -> Result<(), RemoteError>;
    }

    struct DeferredNestedListSubtree;

    #[tool_definition]
    trait DeferredNestedListValueIsParent {
        #[arg(items = "option")]
        #[command(subtree = DeferredNestedListValueIsChild)]
        fn deferred_nested_list_value_is_child(
            &self,
            items: Vec<Vec<u32>>,
        ) -> DeferredNestedListSubtree;
    }

    #[test]
    fn value_is_repeatable_surfaces_do_not_accept_nested_element_literals() {
        let failures = [
            value_is_mismatch_failure(
                __golem_tool_descriptor_for_ValueIsNestedListOption(&mut ToolBuildCtx::new()),
                "items",
                "Vec<Vec<u32>> repeatable option",
            ),
            value_is_mismatch_failure(
                __golem_tool_descriptor_for_ValueIsMapOptionWithListValue(&mut ToolBuildCtx::new()),
                "config",
                "BTreeMap<String, Vec<u32>> repeatable-map option",
            ),
            value_is_mismatch_failure(
                __golem_tool_descriptor_for_ValueIsNestedListTail(&mut ToolBuildCtx::new()),
                "items",
                "Vec<Vec<u32>> tail positional",
            ),
            value_is_mismatch_failure(
                __golem_tool_descriptor_for_DeferredNestedListValueIsParent(
                    &mut ToolBuildCtx::new(),
                ),
                "items",
                "deferred parent-supplied Vec<Vec<u32>> global option",
            ),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

        assert!(
            failures.is_empty(),
            "nested collection value_is literals should be rejected at descriptor build:\n{}",
            failures.join("\n")
        );
    }

    #[tool_definition]
    trait ValueIsRepeatableListWholeCollection {
        #[arg(tags = "option")]
        #[constraint(requires_all = value_is("tags", ["prod"]))]
        fn run(&self, tags: Vec<String>) -> Result<(), RemoteError>;
    }

    #[tool_definition]
    trait ValueIsRepeatableMapWholeCollection {
        #[arg(config = "option")]
        #[constraint(requires_all = value_is("config", [("env", 1)]))]
        fn run(&self, config: BTreeMap<String, u32>) -> Result<(), RemoteError>;
    }

    #[tool_definition]
    trait ValueIsTailWholeCollection {
        #[arg(items = "tail")]
        #[constraint(requires_all = value_is("items", ["prod"]))]
        fn run(&self, items: Vec<String>) -> Result<(), RemoteError>;
    }

    #[test]
    fn value_is_repeatable_and_tail_refs_reject_whole_collection_literals() {
        let failures = [
            value_is_mismatch_failure(
                __golem_tool_descriptor_for_ValueIsRepeatableListWholeCollection(
                    &mut ToolBuildCtx::new(),
                ),
                "tags",
                "Vec<String> repeatable option with a whole-list literal",
            ),
            value_is_mismatch_failure(
                __golem_tool_descriptor_for_ValueIsRepeatableMapWholeCollection(
                    &mut ToolBuildCtx::new(),
                ),
                "config",
                "BTreeMap<String, u32> repeatable-map option with a whole-map literal",
            ),
            value_is_mismatch_failure(
                __golem_tool_descriptor_for_ValueIsTailWholeCollection(&mut ToolBuildCtx::new()),
                "items",
                "Vec<String> tail positional with a whole-list literal",
            ),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

        assert!(
            failures.is_empty(),
            "value_is refs for repeatable options and tail positionals must compare one occurrence/item/value, not the whole collected container:\n{}",
            failures.join("\n")
        );
    }

    fn value_is_mismatch_failure(
        result: Result<ExtendedToolType, ToolBuildError>,
        name: &str,
        context: &str,
    ) -> Option<String> {
        match result {
            Err(ToolBuildError::ValueIsTypeMismatch(actual)) if actual == name => None,
            Err(other) => Some(format!(
                "expected ValueIsTypeMismatch for `{name}` in {context}, got {other:?}"
            )),
            Ok(tool) => {
                let validation = tool.try_to_tool();
                Some(format!(
                    "descriptor accepted scalar value_is for a nested collection occurrence in {context}; runtime validation returned {validation:?}"
                ))
            }
        }
    }

    type StringAliasForNumericRefinement = String;

    #[tool_definition]
    trait NumericRefinementOnStringAlias {
        #[arg(name = "option", unit = "ms")]
        fn run(&self, name: StringAliasForNumericRefinement) -> Result<(), RemoteError>;
    }

    #[test]
    fn numeric_refinement_on_string_type_alias_is_rejected() {
        let result =
            __golem_tool_descriptor_for_NumericRefinementOnStringAlias(&mut ToolBuildCtx::new());

        assert!(
            result.is_err(),
            "numeric refinements on a non-numeric type alias must be rejected instead of being silently dropped; descriptor was {result:#?}",
        );
    }

    type StringAliasForTextRefinement = String;

    #[tool_definition]
    trait TextRefinementOnStringAlias {
        #[arg(name = "option", regex = "a+")]
        fn run(&self, name: StringAliasForTextRefinement) -> Result<(), RemoteError>;
    }

    #[test]
    fn text_refinement_on_string_type_alias_is_applied() {
        // A type alias is macro-opaque, so the macro cannot classify it; the
        // runtime refinement sees the real `String` schema and legitimately
        // promotes it to `Text`. This guards the common alias case against the
        // fallible-refinement change.
        let tool =
            __golem_tool_descriptor_for_TextRefinementOnStringAlias(&mut ToolBuildCtx::new())
                .expect("text refinement on a string alias builds");
        let run = tool
            .commands
            .iter()
            .find(|c| c.name == "run")
            .expect("run command");
        let option = run
            .body
            .as_ref()
            .and_then(|body| body.options.first())
            .expect("option");
        let graph = match &option.shape {
            golem_rust::agentic::ExtendedOptionShape::Scalar(g) => g,
            other => panic!("expected a scalar option shape, got {other:#?}"),
        };
        assert!(
            matches!(graph.root, golem_rust::schema::SchemaType::Text { .. }),
            "a regex-refined string alias should resolve to a Text schema, got {:#?}",
            graph.root
        );
    }

    type StringAliasForUrlRefinement = String;

    #[tool_definition]
    trait UrlRefinementOnStringAlias {
        #[arg(name = "option", schemes = ["https"])]
        fn run(&self, name: StringAliasForUrlRefinement) -> Result<(), RemoteError>;
    }

    #[test]
    fn url_refinement_on_string_type_alias_is_rejected() {
        // A url refinement on a non-url schema must error rather than silently
        // rewriting the String into a Url (the symmetric silent-rewrite case).
        let result =
            __golem_tool_descriptor_for_UrlRefinementOnStringAlias(&mut ToolBuildCtx::new());

        assert!(
            result.is_err(),
            "url refinements on a string alias must be rejected instead of silently rewriting the schema; descriptor was {result:#?}",
        );
    }

    #[tool_definition]
    trait ValueIsRegexRestrictionMismatch {
        #[arg(format = "option", regex = "^(json|yaml)$")]
        #[constraint(requires_all = value_is("format", "toml"))]
        fn run(&self, format: String) -> Result<(), RemoteError>;
    }

    #[test]
    fn value_is_regex_restriction_mismatch_is_rejected_during_descriptor_build() {
        match __golem_tool_descriptor_for_ValueIsRegexRestrictionMismatch(&mut ToolBuildCtx::new())
        {
            Err(ToolBuildError::ValueIsTypeMismatch(name)) if name == "format" => {}
            Err(other) => panic!("expected ValueIsTypeMismatch for `format`, got {other:?}"),
            Ok(tool) => panic!(
                "descriptor accepted a value_is literal that violates the referenced regex restriction; try_to_tool returned {:?}",
                tool.try_to_tool()
            ),
        }
    }

    #[tool_definition]
    trait ValueIsRefinedRepeatableListOption {
        #[arg(tags = "option", regex = "^prod$")]
        #[constraint(requires_all = value_is("tags", "prod"))]
        fn run(&self, tags: Vec<String>) -> Result<(), RemoteError>;
    }

    #[test]
    fn value_is_on_refined_repeatable_list_option_builds() {
        let tool = __golem_tool_descriptor_for_ValueIsRefinedRepeatableListOption(
            &mut ToolBuildCtx::new(),
        )
        .expect(
            "value_is on a refined repeatable-list option should interpret the literal against the refined item type",
        );

        tool.try_to_tool().expect(
            "runtime validation should accept the same refined repeatable-list value_is literal",
        );
    }

    fn cargo_check_tool_crate(name: &str, source: &str) -> std::process::Output {
        let root = std::env::temp_dir().join(format!(
            "golem-rust-tool-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).unwrap();

        let golem_rust_path = Path::new(env!("CARGO_MANIFEST_DIR"));
        fs::write(
            root.join("Cargo.toml"),
            format!(
                r#"
[package]
name = "{name}"
version = "0.0.0"
edition = "2024"

[dependencies]
golem-rust = {{ path = {}, features = ["export_golem_agentic"] }}
"#,
                toml_string(golem_rust_path)
            ),
        )
        .unwrap();
        fs::write(root.join("src/lib.rs"), source).unwrap();

        let target_dir = golem_rust_path
            .parent()
            .expect("golem-rust crate should have an SDK workspace parent")
            .join("target");
        let output = Command::new("cargo")
            .arg("check")
            .arg("--quiet")
            .env("CARGO_TARGET_DIR", target_dir)
            .current_dir(&root)
            .output()
            .expect("failed to run cargo check for temporary tool crate");

        fs::remove_dir_all(&root).unwrap_or_else(|error| {
            panic!(
                "failed to remove temporary workspace {}: {error}",
                root.display()
            )
        });
        output
    }

    fn toml_string(path: &Path) -> String {
        format!("{:?}", path.display().to_string())
    }
}
