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
        CanonicalInputModel, ExtendedOptionShape, ExtendedToolType, ToolBuildCtx, ToolBuildError,
        ToolErrorSchema, get_tool_invoker_by_name,
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

    fn encoded_input(
        tool: &ExtendedToolType,
        command_path: &[&str],
        values: Vec<golem_rust::SchemaValue>,
    ) -> golem_rust::golem_agentic::exports::golem::tool::guest::TypedSchemaValue {
        let path = command_path
            .iter()
            .map(|segment| segment.to_string())
            .collect::<Vec<_>>();
        let command_index = tool
            .command_index_by_path(&path)
            .expect("command path resolves");
        let model = CanonicalInputModel::from_fields(tool.canonical_input_fields(command_index))
            .expect("canonical input model builds");
        let input = golem_rust::TypedSchemaValue::new(
            model.record_schema,
            golem_rust::SchemaValue::Record { fields: values },
        );
        golem_rust::encode_typed_schema_value(&input).expect("typed schema value encodes")
    }

    fn anonymous_principal() -> golem_rust::agentic::Principal {
        golem_rust::agentic::Principal::Anonymous
    }

    #[tool_definition]
    trait PrincipalAutoInjectedRoundTrip {
        fn whoami(&self, principal: golem_rust::agentic::Principal, name: String) -> String;
    }

    #[test]
    fn tool_descriptor_accepts_auto_injected_principal_parameters() {
        let tool = __golem_tool_descriptor_for_PrincipalAutoInjectedRoundTrip(
            &mut ToolBuildCtx::new(),
        )
        .expect("Principal is supplied by the guest invocation principal, not by the input record");
        let command_index = tool
            .command_index_by_path(&["whoami".to_string()])
            .expect("whoami command exists");
        let fields = tool.canonical_input_fields(command_index);

        assert_eq!(
            fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["name"],
            "auto-injected Principal parameters must not appear in the tool input schema",
        );
    }

    #[test]
    fn imported_user_principal_parameter_is_schema_input() {
        mod user_principal_schema {
            use golem_rust::{FromSchema, IntoSchema};

            #[derive(IntoSchema, FromSchema)]
            pub struct Principal {
                pub id: String,
            }
        }

        use user_principal_schema::Principal;

        #[tool_definition]
        trait LocalImportedPrincipalTool {
            fn whoami(&self, principal: Principal, name: String) -> String;
        }

        let tool = __golem_tool_descriptor_for_LocalImportedPrincipalTool(&mut ToolBuildCtx::new())
            .expect("an imported user Principal type is a normal schema input");
        let command_index = tool
            .command_index_by_path(&["whoami".to_string()])
            .expect("whoami command exists");
        let fields = tool.canonical_input_fields(command_index);

        assert_eq!(
            fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["principal", "name"],
            "bare/imported Principal is not enough to mark a tool parameter as auto-injected",
        );
    }

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

    #[tool_definition]
    trait SameTraitGlobalRoundTrip {
        #[arg(verbose = "global", kind = "flag")]
        fn same_trait_global_round_trip(
            &self,
            verbose: bool,
            target: String,
        ) -> Result<(), RemoteError>;

        fn leaf(&self, verbose: bool, name: String) -> String;

        fn fail(&self, reason: String) -> Result<String, RemoteError>;
    }

    struct SameTraitGlobalRoundTripImpl;

    #[tool_implementation]
    impl SameTraitGlobalRoundTrip for SameTraitGlobalRoundTripImpl {
        fn same_trait_global_round_trip(
            &self,
            _verbose: bool,
            _target: String,
        ) -> Result<(), RemoteError> {
            Ok(())
        }

        fn leaf(&self, verbose: bool, name: String) -> String {
            format!("{verbose}:{name}")
        }

        fn fail(&self, reason: String) -> Result<String, RemoteError> {
            Err(RemoteError::Failed(reason))
        }
    }

    #[test]
    fn guest_invoke_decodes_same_trait_root_globals_and_plain_return() {
        let tool = <SameTraitGlobalRoundTripImpl as SameTraitGlobalRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("same-trait-global-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["leaf"],
            vec![
                golem_rust::SchemaValue::Bool(true),
                golem_rust::SchemaValue::String("alice".to_string()),
            ],
        );

        let result = invoker(vec!["leaf".to_string()], input, None, anonymous_principal())
            .expect("guest invocation succeeds");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "true:alice");
    }

    #[test]
    fn guest_invoke_encodes_custom_tool_errors() {
        let tool = <SameTraitGlobalRoundTripImpl as SameTraitGlobalRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("same-trait-global-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["fail"],
            vec![
                golem_rust::SchemaValue::Bool(false),
                golem_rust::SchemaValue::String("boom".to_string()),
            ],
        );

        let err = invoker(vec!["fail".to_string()], input, None, anonymous_principal())
            .expect_err("tool error is surfaced as guest ToolError");
        let golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::CustomError(value) =
            err
        else {
            panic!("expected custom tool error, got {err:?}");
        };
        let value = golem_rust::decode_typed_schema_value(&value).expect("custom error decodes");
        let decoded = RemoteError::from_error_payload_value(value)
            .expect("custom error payload decodes to the declared error enum");
        match decoded {
            RemoteError::Failed(reason) => assert_eq!(reason, "boom"),
        }
    }

    #[test]
    fn guest_custom_tool_error_payload_matches_declared_error_case_payload_schema() {
        let tool = <SameTraitGlobalRoundTripImpl as SameTraitGlobalRoundTrip>::__tool_descriptor();
        let fail_index = tool
            .command_index_by_path(&["fail".to_string()])
            .expect("fail command exists");
        let payload_schema = tool.commands[fail_index]
            .body
            .as_ref()
            .and_then(|body| body.errors.first())
            .and_then(|error| error.payload.as_ref())
            .expect("RemoteError::Failed declares a payload schema");
        assert!(
            matches!(payload_schema.root, golem_rust::SchemaType::String { .. }),
            "the declared error-case payload schema is String, got {:#?}",
            payload_schema.root
        );

        let invoker = get_tool_invoker_by_name("same-trait-global-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["fail"],
            vec![
                golem_rust::SchemaValue::Bool(false),
                golem_rust::SchemaValue::String("boom".to_string()),
            ],
        );

        let err = invoker(vec!["fail".to_string()], input, None, anonymous_principal())
            .expect_err("tool error is surfaced as guest ToolError");
        let golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::CustomError(value) =
            err
        else {
            panic!("expected custom tool error, got {err:?}");
        };
        let value = golem_rust::decode_typed_schema_value(&value).expect("custom error decodes");
        let payload = String::from_value(value.value())
            .expect("custom-error payload must match the declared error-case payload type");
        assert_eq!(payload, "boom");
    }

    #[derive(Debug, Eq, PartialEq, ToolError)]
    enum AmbiguousRemoteError {
        #[tool_error(kind = "usage-error", exit_code = 2)]
        BadInput(String),
        #[tool_error(kind = "runtime-error", exit_code = 1)]
        Backend(String),
    }

    #[tool_definition]
    trait AmbiguousErrorRoundTrip {
        fn fail_backend(&self, reason: String) -> Result<String, AmbiguousRemoteError>;
    }

    struct AmbiguousErrorRoundTripImpl;

    #[tool_implementation]
    impl AmbiguousErrorRoundTrip for AmbiguousErrorRoundTripImpl {
        fn fail_backend(&self, reason: String) -> Result<String, AmbiguousRemoteError> {
            Err(AmbiguousRemoteError::Backend(reason))
        }
    }

    #[test]
    fn custom_tool_error_duplicate_payload_schemas_decode_as_first_matching_case() {
        let tool = <AmbiguousErrorRoundTripImpl as AmbiguousErrorRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("ambiguous-error-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["fail-backend"],
            vec![golem_rust::SchemaValue::String("boom".to_string())],
        );

        let err = invoker(
            vec!["fail-backend".to_string()],
            input,
            None,
            anonymous_principal(),
        )
        .expect_err("tool error is surfaced as guest ToolError");
        let golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::CustomError(value) =
            err
        else {
            panic!("expected custom tool error, got {err:?}");
        };
        let value = golem_rust::decode_typed_schema_value(&value).expect("custom error decodes");

        assert_eq!(
            AmbiguousRemoteError::from_error_payload_value(value)
                .expect("custom error payload decodes"),
            AmbiguousRemoteError::BadInput("boom".to_string()),
            "the current custom-error wire shape carries only the payload, so duplicate payload schemas decode to the first matching case",
        );
    }

    #[test]
    fn bug_finder_guest_custom_error_wire_value_matches_declared_case_payload() {
        let tool = <SameTraitGlobalRoundTripImpl as SameTraitGlobalRoundTrip>::__tool_descriptor();
        let fail_index = tool
            .command_index_by_path(&["fail".to_string()])
            .expect("fail command exists");
        assert!(
            matches!(
                tool.commands[fail_index]
                    .body
                    .as_ref()
                    .and_then(|body| body.errors.first())
                    .and_then(|error| error.payload.as_ref())
                    .map(|payload| &payload.root),
                Some(golem_rust::SchemaType::String { .. })
            ),
            "the declared custom error case payload is String"
        );

        let invoker = get_tool_invoker_by_name("same-trait-global-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["fail"],
            vec![
                golem_rust::SchemaValue::Bool(false),
                golem_rust::SchemaValue::String("declared-payload".to_string()),
            ],
        );

        let err = invoker(vec!["fail".to_string()], input, None, anonymous_principal())
            .expect_err("tool error is surfaced as guest ToolError");
        let golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::CustomError(value) =
            err
        else {
            panic!("expected custom tool error, got {err:?}");
        };
        let value = golem_rust::decode_typed_schema_value(&value).expect("custom error decodes");
        let payload = String::from_value(value.value())
            .expect("custom-error wire value must match the declared error-case payload schema");
        assert_eq!(payload, "declared-payload");
    }

    #[derive(Debug, Eq, PartialEq, IntoSchema, FromSchema)]
    struct CustomPlainReturn {
        name: String,
    }

    #[tool_definition]
    trait CustomPlainReturnRoundTrip {
        fn custom_plain(&self, name: String) -> CustomPlainReturn;
    }

    struct CustomPlainReturnRoundTripImpl;

    #[tool_implementation]
    impl CustomPlainReturnRoundTrip for CustomPlainReturnRoundTripImpl {
        fn custom_plain(&self, name: String) -> CustomPlainReturn {
            CustomPlainReturn { name }
        }
    }

    #[test]
    fn guest_invoke_encodes_custom_plain_return_types() {
        let tool =
            <CustomPlainReturnRoundTripImpl as CustomPlainReturnRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("custom-plain-return-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["custom-plain"],
            vec![golem_rust::SchemaValue::String("custom".to_string())],
        );

        let result = invoker(
            vec!["custom-plain".to_string()],
            input,
            None,
            anonymous_principal(),
        )
        .expect("guest invocation succeeds for a custom IntoSchema return type");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = CustomPlainReturn::from_value(result.value())
            .expect("result schema matches return type");
        assert_eq!(
            value,
            CustomPlainReturn {
                name: "custom".to_string()
            }
        );
    }

    #[tool_definition]
    trait DefaultBodyInvokeRoundTrip {
        fn default_body_invoke_round_trip(&self, name: String) -> String {
            format!("default:{name}")
        }
    }

    struct DefaultBodyInvokeRoundTripImpl;

    #[tool_implementation]
    impl DefaultBodyInvokeRoundTrip for DefaultBodyInvokeRoundTripImpl {}

    #[test]
    fn guest_invoke_dispatches_trait_default_method_bodies() {
        let tool =
            <DefaultBodyInvokeRoundTripImpl as DefaultBodyInvokeRoundTrip>::__tool_descriptor();
        assert!(
            tool.command_index_by_path(&[]).is_some(),
            "the default method body is present in the generated descriptor"
        );
        let invoker = get_tool_invoker_by_name("default-body-invoke-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &[],
            vec![golem_rust::SchemaValue::String("alice".to_string())],
        );

        let result = invoker(vec![], input, None, anonymous_principal())
            .expect("guest invocation dispatches the trait default method body");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "default:alice");
    }

    #[test]
    fn stdout_tool_invocation_shape_compiles() {
        let output = cargo_check_tool_crate(
            "stdout-tool-invocation-shape",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum RemoteError {
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Failed(String),
}

#[tool_definition]
trait StdoutTool {
    fn run(&self, stdout: golem_rust::wasip2::io::streams::OutputStream) -> Result<(), RemoteError>;
}

struct StdoutToolImpl;

#[tool_implementation]
impl StdoutTool for StdoutToolImpl {
    fn run(&self, _stdout: golem_rust::wasip2::io::streams::OutputStream) -> Result<(), RemoteError> {
        Ok(())
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "stdout tool definitions should generate a guest invoker and typed client shape, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[tool_definition]
    trait ChildRoundTrip {
        fn leaf(&self, verbose: bool, name: String) -> String;
    }

    struct ChildRoundTripSubtree;
    struct ChildRoundTripImpl;

    #[tool_implementation]
    impl ChildRoundTrip for ChildRoundTripImpl {
        fn leaf(&self, verbose: bool, name: String) -> String {
            format!("child:{verbose}:{name}")
        }
    }

    #[tool_definition]
    trait ParentRoundTrip {
        #[command(name = "kid", aliases = ["k"], subtree = ChildRoundTrip)]
        fn child_round_trip(&self, verbose: bool) -> ChildRoundTripSubtree;
    }

    struct ParentRoundTripImpl;

    #[tool_implementation]
    impl ParentRoundTrip for ParentRoundTripImpl {
        fn child_round_trip(&self, _verbose: bool) -> ChildRoundTripSubtree {
            ChildRoundTripSubtree
        }
    }

    #[test]
    fn guest_invoke_routes_grafted_subtrees_to_child_invoker() {
        let tool = <ParentRoundTripImpl as ParentRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("parent-round-trip")
            .expect("parent tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["kid", "leaf"],
            vec![
                golem_rust::SchemaValue::Bool(true),
                golem_rust::SchemaValue::String("bob".to_string()),
            ],
        );

        let result = invoker(
            vec!["kid".to_string(), "leaf".to_string()],
            input,
            None,
            anonymous_principal(),
        )
        .expect("grafted subtree invocation succeeds");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "child:true:bob");
    }

    #[test]
    fn guest_invoke_routes_grafted_subtrees_through_command_aliases() {
        let tool = <ParentRoundTripImpl as ParentRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("parent-round-trip")
            .expect("parent tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["k", "leaf"],
            vec![
                golem_rust::SchemaValue::Bool(true),
                golem_rust::SchemaValue::String("aliased".to_string()),
            ],
        );

        let result = invoker(
            vec!["k".to_string(), "leaf".to_string()],
            input,
            None,
            anonymous_principal(),
        )
        .expect("grafted subtree invocation succeeds through command aliases");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "child:true:aliased");
    }

    #[tool_definition]
    trait AliasChildRoundTrip {
        fn leaf(&self, format: u32, name: String) -> String;
    }

    struct AliasChildRoundTripSubtree;
    struct AliasChildRoundTripImpl;

    #[tool_implementation]
    impl AliasChildRoundTrip for AliasChildRoundTripImpl {
        fn leaf(&self, format: u32, name: String) -> String {
            format!("child:{format}:{name}")
        }
    }

    #[tool_definition]
    trait AliasParentSubtreeRoundTrip {
        #[arg(count = "global", aliases = ["format"])]
        #[command(subtree = AliasChildRoundTrip)]
        fn alias_child_round_trip(&self, count: u32) -> AliasChildRoundTripSubtree;
    }

    struct AliasParentSubtreeRoundTripImpl;

    #[tool_implementation]
    impl AliasParentSubtreeRoundTrip for AliasParentSubtreeRoundTripImpl {
        fn alias_child_round_trip(&self, _count: u32) -> AliasChildRoundTripSubtree {
            AliasChildRoundTripSubtree
        }
    }

    #[test]
    fn guest_invoke_maps_alias_deprojected_subtree_globals_to_child_parameters() {
        let tool =
            <AliasParentSubtreeRoundTripImpl as AliasParentSubtreeRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("alias-parent-subtree-round-trip")
            .expect("parent tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["alias-child-round-trip", "leaf"],
            vec![
                golem_rust::SchemaValue::U32(7),
                golem_rust::SchemaValue::String("aliased-subtree".to_string()),
            ],
        );

        let result = invoker(
            vec!["alias-child-round-trip".to_string(), "leaf".to_string()],
            input,
            None,
            anonymous_principal(),
        )
        .expect(
            "guest invocation maps an inherited subtree global alias back to the child parameter",
        );
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "child:7:aliased-subtree");
    }

    #[tool_definition]
    trait EarlierGlobalAliasParentSubtreeRoundTrip {
        #[arg(profile = "global")]
        fn earlier_global_alias_parent_subtree_round_trip(
            &self,
            profile: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(count = "global", aliases = ["format"])]
        #[command(name = "alias-child-round-trip", aliases = ["alias-child"], subtree = AliasChildRoundTrip)]
        fn alias_child(&self, count: u32) -> AliasChildRoundTripSubtree;
    }

    struct EarlierGlobalAliasParentSubtreeRoundTripImpl;

    #[tool_implementation]
    impl EarlierGlobalAliasParentSubtreeRoundTrip for EarlierGlobalAliasParentSubtreeRoundTripImpl {
        fn earlier_global_alias_parent_subtree_round_trip(
            &self,
            _profile: u32,
            _target: String,
        ) -> Result<(), RemoteError> {
            Ok(())
        }

        fn alias_child(&self, _count: u32) -> AliasChildRoundTripSubtree {
            AliasChildRoundTripSubtree
        }
    }

    #[test]
    fn guest_invoke_uses_alias_mapping_not_position_for_subtree_globals() {
        let tool = <EarlierGlobalAliasParentSubtreeRoundTripImpl as EarlierGlobalAliasParentSubtreeRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("earlier-global-alias-parent-subtree-round-trip")
            .expect("parent tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["alias-child", "leaf"],
            vec![
                golem_rust::SchemaValue::U32(99),
                golem_rust::SchemaValue::U32(7),
                golem_rust::SchemaValue::String("aliased-subtree".to_string()),
            ],
        );

        let result = invoker(
            vec!["alias-child".to_string(), "leaf".to_string()],
            input,
            None,
            anonymous_principal(),
        )
        .expect("guest invocation uses alias mapping instead of same-typed absolute position");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "child:7:aliased-subtree");
    }

    #[tool_definition]
    trait ReverseAliasChildRoundTrip {
        #[arg(count = "option", aliases = ["format"])]
        fn leaf(&self, count: u32, name: String) -> String;
    }

    struct ReverseAliasChildRoundTripSubtree;
    struct ReverseAliasChildRoundTripImpl;

    #[tool_implementation]
    impl ReverseAliasChildRoundTrip for ReverseAliasChildRoundTripImpl {
        fn leaf(&self, count: u32, name: String) -> String {
            format!("child:{count}:{name}")
        }
    }

    #[tool_definition]
    trait ReverseAliasParentSubtreeRoundTrip {
        #[arg(format = "global")]
        #[command(subtree = ReverseAliasChildRoundTrip)]
        fn reverse_alias_child_round_trip(&self, format: u32) -> ReverseAliasChildRoundTripSubtree;
    }

    struct ReverseAliasParentSubtreeRoundTripImpl;

    #[tool_implementation]
    impl ReverseAliasParentSubtreeRoundTrip for ReverseAliasParentSubtreeRoundTripImpl {
        fn reverse_alias_child_round_trip(
            &self,
            _format: u32,
        ) -> ReverseAliasChildRoundTripSubtree {
            ReverseAliasChildRoundTripSubtree
        }
    }

    #[test]
    fn guest_invoke_matches_child_aliases_for_subtree_globals() {
        let tool = <ReverseAliasParentSubtreeRoundTripImpl as ReverseAliasParentSubtreeRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("reverse-alias-parent-subtree-round-trip")
            .expect("parent tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["reverse-alias-child-round-trip", "leaf"],
            vec![
                golem_rust::SchemaValue::U32(7),
                golem_rust::SchemaValue::String("reverse-alias".to_string()),
            ],
        );

        let result = invoker(
            vec![
                "reverse-alias-child-round-trip".to_string(),
                "leaf".to_string(),
            ],
            input,
            None,
            anonymous_principal(),
        )
        .expect("guest invocation maps a child alias back to the inherited parent global");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "child:7:reverse-alias");
    }

    #[tool_definition]
    trait ImplParameterNameRoundTrip {
        fn leaf(&self, verbose: bool, name: String) -> String;
    }

    struct ImplParameterNameRoundTripImpl;

    #[tool_implementation]
    impl ImplParameterNameRoundTrip for ImplParameterNameRoundTripImpl {
        fn leaf(&self, v: bool, n: String) -> String {
            format!("{v}:{n}")
        }
    }

    #[test]
    fn guest_invoke_uses_trait_parameter_names_not_impl_parameter_names() {
        let tool =
            <ImplParameterNameRoundTripImpl as ImplParameterNameRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("impl-parameter-name-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["leaf"],
            vec![
                golem_rust::SchemaValue::String("renamed".to_string()),
                golem_rust::SchemaValue::Bool(true),
            ],
        );

        let result = invoker(vec!["leaf".to_string()], input, None, anonymous_principal())
            .expect("guest invocation succeeds when impl parameter names differ from the trait");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "true:renamed");
    }

    #[tool_definition]
    trait AliasInheritedGlobalRoundTrip {
        #[arg(count = "global", aliases = ["format"])]
        fn alias_inherited_global_round_trip(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        fn leaf(&self, format: u32, name: String) -> String;
    }

    struct AliasInheritedGlobalRoundTripImpl;

    #[tool_implementation]
    impl AliasInheritedGlobalRoundTrip for AliasInheritedGlobalRoundTripImpl {
        fn alias_inherited_global_round_trip(
            &self,
            _count: u32,
            _target: String,
        ) -> Result<(), RemoteError> {
            Ok(())
        }

        fn leaf(&self, format: u32, name: String) -> String {
            format!("{format}:{name}")
        }
    }

    #[test]
    fn guest_invoke_maps_alias_deprojected_inherited_globals_to_method_parameters() {
        let tool =
            <AliasInheritedGlobalRoundTripImpl as AliasInheritedGlobalRoundTrip>::__tool_descriptor(
            );
        let invoker = get_tool_invoker_by_name("alias-inherited-global-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["leaf"],
            vec![
                golem_rust::SchemaValue::U32(7),
                golem_rust::SchemaValue::String("aliased-global".to_string()),
            ],
        );

        let result = invoker(vec!["leaf".to_string()], input, None, anonymous_principal())
            .expect("guest invocation maps an inherited global alias back to the method parameter");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "7:aliased-global");
    }

    #[tool_definition]
    trait ReverseAliasInheritedGlobalRoundTrip {
        #[arg(format = "global")]
        fn reverse_alias_inherited_global_round_trip(
            &self,
            format: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(count = "option", aliases = ["format"])]
        fn leaf(&self, count: u32, name: String) -> String;
    }

    struct ReverseAliasInheritedGlobalRoundTripImpl;

    #[tool_implementation]
    impl ReverseAliasInheritedGlobalRoundTrip for ReverseAliasInheritedGlobalRoundTripImpl {
        fn reverse_alias_inherited_global_round_trip(
            &self,
            _format: u32,
            _target: String,
        ) -> Result<(), RemoteError> {
            Ok(())
        }

        fn leaf(&self, count: u32, name: String) -> String {
            format!("{count}:{name}")
        }
    }

    #[test]
    fn guest_invoke_matches_child_aliases_for_same_trait_inherited_globals() {
        let tool = <ReverseAliasInheritedGlobalRoundTripImpl as ReverseAliasInheritedGlobalRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("reverse-alias-inherited-global-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["leaf"],
            vec![
                golem_rust::SchemaValue::U32(7),
                golem_rust::SchemaValue::String("reverse-alias-global".to_string()),
            ],
        );

        let result = invoker(vec!["leaf".to_string()], input, None, anonymous_principal())
            .expect("guest invocation maps a child alias back to the same-trait inherited global");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "7:reverse-alias-global");
    }

    #[tool_definition]
    trait RootBodyAliasDoesNotDeprojectRoundTrip {
        #[arg(count = "option", aliases = ["format"])]
        fn root_body_alias_does_not_deproject_round_trip(
            &self,
            count: u32,
        ) -> Result<(), RemoteError>;

        fn leaf(&self, format: u32, name: String) -> String;
    }

    struct RootBodyAliasDoesNotDeprojectRoundTripImpl;

    #[tool_implementation]
    impl RootBodyAliasDoesNotDeprojectRoundTrip for RootBodyAliasDoesNotDeprojectRoundTripImpl {
        fn root_body_alias_does_not_deproject_round_trip(
            &self,
            _count: u32,
        ) -> Result<(), RemoteError> {
            Ok(())
        }

        fn leaf(&self, format: u32, name: String) -> String {
            format!("{format}:{name}")
        }
    }

    #[test]
    fn guest_invoke_does_not_deproject_root_body_aliases() {
        let tool = <RootBodyAliasDoesNotDeprojectRoundTripImpl as RootBodyAliasDoesNotDeprojectRoundTrip>::__tool_descriptor();
        let invoker = get_tool_invoker_by_name("root-body-alias-does-not-deproject-round-trip")
            .expect("tool implementation registers an invoker");
        let input = encoded_input(
            &tool,
            &["leaf"],
            vec![
                golem_rust::SchemaValue::U32(7),
                golem_rust::SchemaValue::String("leaf-value".to_string()),
            ],
        );

        let result = invoker(vec!["leaf".to_string()], input, None, anonymous_principal())
            .expect("root body aliases are not inherited by sibling commands");
        let result = result.result.expect("plain return is encoded as a result");
        let result = golem_rust::decode_typed_schema_value(&result).expect("result decodes");
        let value = String::from_value(result.value()).expect("result schema matches String");
        assert_eq!(value, "7:leaf-value");
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

    #[tool_definition]
    trait RootBodyAliasCollisionChild {
        #[arg(format = "option", aliases = ["count"])]
        #[constraint(requires_all = value_is("count", 1))]
        fn root_body_alias_collision_child(&self, format: String) -> Result<(), RemoteError>;
    }

    struct RootBodyAliasCollisionSubtree;

    #[tool_definition]
    trait RootBodyAliasCollisionParent {
        #[arg(count = "global")]
        fn root_body_alias_collision_parent(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = RootBodyAliasCollisionChild)]
        fn root_body_alias_collision_child(&self, format: String) -> RootBodyAliasCollisionSubtree;
    }

    #[test]
    fn grafted_root_body_alias_collision_with_strict_ancestor_is_rejected() {
        let err = __golem_tool_descriptor_for_RootBodyAliasCollisionParent(&mut ToolBuildCtx::new())
            .expect_err(
                "a grafted root body option colliding with the subtree global and an incompatible strict ancestor global must be rejected",
            );
        assert!(
            matches!(
                err,
                ToolBuildError::InheritedGlobalConflict { ref name, ref inherited, ref command }
                    if name == "count"
                        && inherited == "count"
                        && command == "root-body-alias-collision-child"
            ),
            "expected an InheritedGlobalConflict for alias `count` on the grafted root body, got {err:?}",
        );
    }

    #[tool_definition]
    trait RootBodyTailInferenceChild {
        fn root_body_tail_inference_child(
            &self,
            items: Vec<String>,
            format: String,
        ) -> Result<(), RemoteError>;
    }

    struct RootBodyTailInferenceSubtree;

    #[tool_definition]
    trait RootBodyTailInferenceParent {
        #[command(subtree = RootBodyTailInferenceChild)]
        fn root_body_tail_inference_child(&self, format: String) -> RootBodyTailInferenceSubtree;
    }

    #[test]
    fn grafted_root_body_deprojection_preserves_tail_inference() {
        let tool =
            __golem_tool_descriptor_for_RootBodyTailInferenceParent(&mut ToolBuildCtx::new())
                .expect("descriptor builds");

        let child_idx = tool.commands[0]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| tool.commands[idx as usize].name == "root-body-tail-inference-child")
            .expect("child subtree is grafted") as usize;
        let body = tool.commands[child_idx]
            .body
            .as_ref()
            .expect("grafted root body is preserved");

        assert!(
            !body.positionals.fixed.iter().any(|p| p.name == "format")
                && !body.options.iter().any(|o| o.long == "format"),
            "root-body-local `format` must be removed because the subtree method supplies it as an inherited global",
        );
        assert_eq!(
            body.positionals.tail.as_ref().map(|t| t.name.as_str()),
            Some("items"),
            "the trailing inherited-global re-declaration must not prevent `items` from being inferred as the tail positional on a grafted root body",
        );
        assert!(
            !body.options.iter().any(|o| o.long == "items"),
            "`items` should not be projected as a repeatable-list option when the only later positional is de-projected",
        );
        tool.try_to_tool().expect("composed tool is valid");
    }

    #[tool_definition]
    trait OverrideConflictChild {
        #[arg(verbose = "option")]
        fn override_conflict_child(&self, verbose: String) -> Result<(), RemoteError>;
    }

    struct OverrideConflictSubtree;

    #[tool_definition]
    trait OverrideConflictParent {
        #[command(subtree = OverrideConflictChild, name = "renamed")]
        fn child(&self, verbose: bool) -> OverrideConflictSubtree;
    }

    #[test]
    fn grafted_root_body_conflict_reports_overridden_command_name() {
        let err = __golem_tool_descriptor_for_OverrideConflictParent(&mut ToolBuildCtx::new())
            .expect_err("incompatible inherited global must be rejected");
        assert!(
            matches!(
                err,
                ToolBuildError::InheritedGlobalConflict { ref name, ref inherited, ref command }
                    if name == "verbose" && inherited == "verbose" && command == "renamed"
            ),
            "expected the inherited-global conflict to be reported against the final overridden command name `renamed`, got {err:?}",
        );
    }

    #[tool_definition]
    trait MismatchedConflictChild {
        #[arg(verbose = "option")]
        fn mismatched_conflict_child(&self, verbose: String) -> Result<(), RemoteError>;
    }

    struct MismatchedConflictSubtree;

    #[tool_definition]
    trait MismatchedConflictParent {
        #[command(subtree = MismatchedConflictChild)]
        fn renamed(&self, verbose: bool) -> MismatchedConflictSubtree;
    }

    #[test]
    fn subtree_root_name_mismatch_is_reported_before_inherited_global_conflict() {
        let err = __golem_tool_descriptor_for_MismatchedConflictParent(&mut ToolBuildCtx::new())
            .expect_err(
                "a subtree root whose name differs from the parent command must be rejected",
            );
        assert!(
            matches!(
                err,
                ToolBuildError::SubtreeRootNameMismatch { ref expected, ref actual }
                    if expected == "renamed" && actual == "mismatched-conflict-child"
            ),
            "expected SubtreeRootNameMismatch to win before inherited-global reconciliation, got {err:?}",
        );
    }

    #[tool_definition]
    trait MismatchedParentGlobalConflictChild {
        fn mismatched_parent_global_conflict_child(&self, value: String)
        -> Result<(), RemoteError>;
    }

    struct MismatchedParentGlobalConflictSubtree;

    #[tool_definition]
    trait MismatchedParentGlobalConflictParent {
        #[arg(verbose = "global")]
        fn mismatched_parent_global_conflict_parent(
            &self,
            verbose: String,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = MismatchedParentGlobalConflictChild)]
        fn renamed(&self, verbose: bool) -> MismatchedParentGlobalConflictSubtree;
    }

    #[test]
    fn subtree_root_name_mismatch_beats_parent_global_reconciliation() {
        let err = __golem_tool_descriptor_for_MismatchedParentGlobalConflictParent(
            &mut ToolBuildCtx::new(),
        )
        .expect_err("a subtree root whose name differs from the parent command must be rejected");
        assert!(
            matches!(
                err,
                ToolBuildError::SubtreeRootNameMismatch { ref expected, ref actual }
                    if expected == "renamed" && actual == "mismatched-parent-global-conflict-child"
            ),
            "expected SubtreeRootNameMismatch to win before parent-global reconciliation, got {err:?}",
        );
    }

    #[tool_definition]
    trait AliasDeprojectedNestedLeaf {
        #[arg(format = "option")]
        fn alias_deprojected_nested_leaf(&self, format: String) -> Result<(), RemoteError>;
    }

    struct AliasDeprojectedNestedLeafSubtree;

    #[tool_definition]
    trait AliasDeprojectedNestedMiddle {
        #[command(subtree = AliasDeprojectedNestedLeaf)]
        fn alias_deprojected_nested_leaf(&self) -> AliasDeprojectedNestedLeafSubtree;
    }

    struct AliasDeprojectedNestedMiddleSubtree;

    #[tool_definition]
    trait AliasDeprojectedNestedParent {
        #[arg(count = "global")]
        fn alias_deprojected_nested_parent(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = AliasDeprojectedNestedMiddle)]
        #[arg(format = "global", aliases = ["count"])]
        fn alias_deprojected_nested_middle(
            &self,
            format: u32,
        ) -> AliasDeprojectedNestedMiddleSubtree;
    }

    #[test]
    fn deprojected_subtree_global_alias_does_not_shadow_nested_root_body() {
        let tool = __golem_tool_descriptor_for_AliasDeprojectedNestedParent(&mut ToolBuildCtx::new())
            .expect(
                "a subtree global de-projected through an alias must not remain effective under nested grafts",
            );
        let middle_idx = tool.commands[0]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| tool.commands[idx as usize].name == "alias-deprojected-nested-middle")
            .expect("middle subtree is grafted") as usize;
        assert!(
            !tool.commands[middle_idx]
                .globals
                .options
                .iter()
                .any(|o| o.long == "format"),
            "the alias-compatible subtree global is de-projected and must not survive as `format`",
        );
        let leaf_idx = tool.commands[middle_idx]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| tool.commands[idx as usize].name == "alias-deprojected-nested-leaf")
            .expect("leaf subtree is grafted") as usize;
        let leaf_body = tool.commands[leaf_idx].body.as_ref().expect("leaf body");
        assert!(
            leaf_body.options.iter().any(|o| o.long == "format"),
            "the nested leaf's local `format` option must remain because no effective inherited global named `format` survives",
        );
    }

    #[tool_definition]
    trait AliasDeprojectedRootGlobalNestedLeaf {
        #[arg(format = "option")]
        fn alias_deprojected_root_global_nested_leaf(
            &self,
            format: String,
        ) -> Result<(), RemoteError>;
    }

    struct AliasDeprojectedRootGlobalNestedLeafSubtree;

    #[tool_definition]
    trait AliasDeprojectedRootGlobalNestedMiddle {
        #[arg(format = "global", aliases = ["count"])]
        fn alias_deprojected_root_global_nested_middle(
            &self,
            format: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = AliasDeprojectedRootGlobalNestedLeaf)]
        fn alias_deprojected_root_global_nested_leaf(
            &self,
        ) -> AliasDeprojectedRootGlobalNestedLeafSubtree;
    }

    struct AliasDeprojectedRootGlobalNestedMiddleSubtree;

    #[tool_definition]
    trait AliasDeprojectedRootGlobalNestedParent {
        #[arg(count = "global")]
        fn alias_deprojected_root_global_nested_parent(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = AliasDeprojectedRootGlobalNestedMiddle)]
        fn alias_deprojected_root_global_nested_middle(
            &self,
        ) -> AliasDeprojectedRootGlobalNestedMiddleSubtree;
    }

    #[test]
    fn deprojected_child_root_global_alias_does_not_shadow_nested_root_body() {
        let tool = __golem_tool_descriptor_for_AliasDeprojectedRootGlobalNestedParent(
            &mut ToolBuildCtx::new(),
        )
        .expect(
            "a child root global de-projected through an alias must not remain effective under nested grafts",
        );
        let middle_idx = tool.commands[0]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| {
                tool.commands[idx as usize].name == "alias-deprojected-root-global-nested-middle"
            })
            .expect("middle subtree is grafted") as usize;
        assert!(
            !tool.commands[middle_idx]
                .globals
                .options
                .iter()
                .any(|o| o.long == "format"),
            "the alias-compatible child root global is de-projected and must not survive as `format`",
        );
        let leaf_idx = tool.commands[middle_idx]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| {
                tool.commands[idx as usize].name == "alias-deprojected-root-global-nested-leaf"
            })
            .expect("leaf subtree is grafted") as usize;
        let leaf_body = tool.commands[leaf_idx].body.as_ref().expect("leaf body");
        assert!(
            leaf_body.options.iter().any(|o| o.long == "format"),
            "the nested leaf's local `format` option must remain because no effective inherited global named `format` survives",
        );
    }

    #[tool_definition]
    trait AliasDeprojectedRootGlobalTailChild {
        #[arg(format = "global", aliases = ["count"])]
        fn alias_deprojected_root_global_tail_child(&self, format: u32) -> Result<(), RemoteError>;

        fn leaf(&self, items: Vec<String>, format: u32) -> Result<(), RemoteError>;
    }

    struct AliasDeprojectedRootGlobalTailSubtree;

    #[tool_definition]
    trait AliasDeprojectedRootGlobalTailParent {
        #[arg(count = "global")]
        fn alias_deprojected_root_global_tail_parent(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = AliasDeprojectedRootGlobalTailChild)]
        fn alias_deprojected_root_global_tail_child(&self)
        -> AliasDeprojectedRootGlobalTailSubtree;
    }

    #[test]
    fn deprojected_child_root_global_alias_does_not_affect_leaf_tail_inference() {
        let tool = __golem_tool_descriptor_for_AliasDeprojectedRootGlobalTailParent(
            &mut ToolBuildCtx::new(),
        )
        .expect("descriptor builds");

        let child_idx = tool.commands[0]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| {
                tool.commands[idx as usize].name == "alias-deprojected-root-global-tail-child"
            })
            .expect("child subtree is grafted") as usize;
        assert!(
            !tool.commands[child_idx]
                .globals
                .options
                .iter()
                .any(|o| o.long == "format"),
            "the child root global `format` is de-projected through alias `count` and must not survive",
        );

        let leaf_idx = tool.commands[child_idx]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| tool.commands[idx as usize].name == "leaf")
            .expect("leaf command is present") as usize;
        let body = tool.commands[leaf_idx].body.as_ref().expect("leaf body");

        assert!(
            body.positionals.fixed.iter().any(|p| p.name == "format"),
            "leaf-local `format` remains because no effective inherited global named `format` survives",
        );
        assert!(
            body.positionals.tail.is_none(),
            "`items: Vec<_>` is followed by surviving local `format`, so it is not in tail position",
        );
        assert!(
            body.options.iter().any(|o| {
                o.long == "items" && matches!(o.shape, ExtendedOptionShape::RepeatableList(_))
            }),
            "a non-tail Vec parameter projects as a repeatable-list option",
        );
    }

    #[tool_definition]
    trait DemotedTailOptionOrderChild {
        #[arg(format = "global", aliases = ["count"])]
        fn demoted_tail_option_order_child(
            &self,
            format: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(mode = "option")]
        fn leaf(&self, items: Vec<String>, mode: String, format: u32) -> Result<(), RemoteError>;
    }

    struct DemotedTailOptionOrderSubtree;

    #[tool_definition]
    trait DemotedTailOptionOrderParent {
        #[arg(count = "global")]
        fn demoted_tail_option_order_parent(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = DemotedTailOptionOrderChild)]
        fn demoted_tail_option_order_child(&self) -> DemotedTailOptionOrderSubtree;
    }

    #[test]
    fn demoted_tail_option_is_reinserted_in_declaration_order() {
        let tool =
            __golem_tool_descriptor_for_DemotedTailOptionOrderParent(&mut ToolBuildCtx::new())
                .expect("descriptor builds");

        let leaf = tool
            .commands
            .iter()
            .find(|command| command.name == "leaf")
            .expect("leaf command");
        let body = leaf.body.as_ref().expect("leaf body");

        assert!(
            body.positionals.tail.is_none(),
            "items must be demoted because leaf-local format survives",
        );
        let option_names: Vec<&str> = body
            .options
            .iter()
            .map(|option| option.long.as_str())
            .collect();
        assert_eq!(
            option_names,
            vec!["items", "mode"],
            "demoting items from tail to repeatable-list option should preserve declaration order among body options",
        );
    }

    #[tool_definition]
    trait DemotedTailMinZeroChild {
        #[arg(format = "global", aliases = ["count"])]
        fn demoted_tail_min_zero_child(
            &self,
            format: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(items, min = 0)]
        fn leaf(&self, items: Vec<String>, format: u32) -> Result<(), RemoteError>;
    }

    struct DemotedTailMinZeroSubtree;

    #[tool_definition]
    trait DemotedTailMinZeroParent {
        #[arg(count = "global")]
        fn demoted_tail_min_zero_parent(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = DemotedTailMinZeroChild)]
        fn demoted_tail_min_zero_child(&self) -> DemotedTailMinZeroSubtree;
    }

    #[test]
    fn demoted_tail_with_authored_min_zero_is_rejected() {
        let err = __golem_tool_descriptor_for_DemotedTailMinZeroParent(&mut ToolBuildCtx::new())
            .expect_err("authored min/max cannot be reinterpreted when demoting a tail");

        assert!(
            matches!(err, ToolBuildError::VecSurfaceConflict { ref name, .. } if name == "items"),
            "expected VecSurfaceConflict for demoted items with authored min = 0, got {err:?}",
        );
    }

    #[tool_definition]
    trait ExplicitTailBeforeSurvivingLocalChild {
        #[arg(format = "global", aliases = ["count"])]
        fn explicit_tail_before_surviving_local_child(
            &self,
            format: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(items = "tail")]
        fn leaf(&self, items: Vec<String>, format: u32) -> Result<(), RemoteError>;
    }

    struct ExplicitTailBeforeSurvivingLocalSubtree;

    #[tool_definition]
    trait ExplicitTailBeforeSurvivingLocalParent {
        #[arg(count = "global")]
        fn explicit_tail_before_surviving_local_parent(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = ExplicitTailBeforeSurvivingLocalChild)]
        fn explicit_tail_before_surviving_local_child(
            &self,
        ) -> ExplicitTailBeforeSurvivingLocalSubtree;
    }

    #[test]
    fn deprojected_child_root_global_alias_revalidates_explicit_tail_order() {
        let result = __golem_tool_descriptor_for_ExplicitTailBeforeSurvivingLocalParent(
            &mut ToolBuildCtx::new(),
        );
        let Ok(tool) = result else {
            return;
        };

        assert!(
            tool.try_to_tool().is_err(),
            "once child root global `format` is de-projected via alias `count`, leaf-local `format` survives; explicitly-authored tail `items` before that fixed positional must be rejected",
        );
    }

    #[tool_definition]
    trait ExplicitInheritedTailSurrogateChild {
        #[arg(items = "global", aliases = ["count"])]
        fn explicit_inherited_tail_surrogate_child(
            &self,
            items: Vec<String>,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(items = "tail")]
        fn leaf(&self, items: Vec<String>, format: String) -> Result<(), RemoteError>;
    }

    struct ExplicitInheritedTailSurrogateSubtree;

    #[tool_definition]
    trait ExplicitInheritedTailSurrogateParent {
        #[arg(count = "global")]
        fn explicit_inherited_tail_surrogate_parent(
            &self,
            count: Vec<String>,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = ExplicitInheritedTailSurrogateChild)]
        fn explicit_inherited_tail_surrogate_child(&self) -> ExplicitInheritedTailSurrogateSubtree;
    }

    #[test]
    fn explicit_inherited_tail_surrogate_before_surviving_positional_is_rejected() {
        let result = __golem_tool_descriptor_for_ExplicitInheritedTailSurrogateParent(
            &mut ToolBuildCtx::new(),
        );
        let err = match result {
            Ok(tool) => tool.try_to_tool().expect_err(
                "an explicitly-authored tail lowered through an inherited-global surrogate must still be rejected when a later fixed positional survives",
            ),
            Err(err) => err,
        };

        assert!(
            matches!(err, ToolBuildError::FixedPositionalAfterTail(ref name) if name == "format"),
            "expected FixedPositionalAfterTail for surviving positional after explicit tail, got {err:?}",
        );
    }

    #[tool_definition]
    trait ExplicitInheritedTailSurrogateAttrsChild {
        #[arg(items = "global", aliases = ["count"])]
        fn explicit_inherited_tail_surrogate_attrs_child(
            &self,
            items: Vec<String>,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(items = "tail", separator = "--", accepts_stdio = true)]
        fn leaf(&self, items: Vec<String>) -> Result<(), RemoteError>;
    }

    struct ExplicitInheritedTailSurrogateAttrsSubtree;

    #[tool_definition]
    trait ExplicitInheritedTailSurrogateAttrsParent {
        #[arg(count = "global")]
        fn explicit_inherited_tail_surrogate_attrs_parent(
            &self,
            count: Vec<String>,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = ExplicitInheritedTailSurrogateAttrsChild)]
        fn explicit_inherited_tail_surrogate_attrs_child(
            &self,
        ) -> ExplicitInheritedTailSurrogateAttrsSubtree;
    }

    #[test]
    fn promoted_explicit_inherited_tail_surrogate_preserves_tail_attrs() {
        let tool = __golem_tool_descriptor_for_ExplicitInheritedTailSurrogateAttrsParent(
            &mut ToolBuildCtx::new(),
        )
        .expect("descriptor builds");
        let leaf = tool
            .commands
            .iter()
            .find(|command| command.name == "leaf")
            .expect("leaf command");
        let tail = leaf
            .body
            .as_ref()
            .and_then(|body| body.positionals.tail.as_ref())
            .expect("items survives as the leaf tail");

        assert_eq!(tail.name, "items");
        assert_eq!(
            tail.separator.as_deref(),
            Some("--"),
            "a surviving explicit tail surrogate must keep authored tail separator",
        );
        assert!(
            tail.accepts_stdio,
            "a surviving explicit tail surrogate must keep authored accepts_stdio",
        );
        tool.try_to_tool().expect("tool is valid");
    }

    #[tool_definition]
    trait ExplicitInheritedTailSurrogateVerbatimChild {
        #[arg(items = "global", aliases = ["count"])]
        fn explicit_inherited_tail_surrogate_verbatim_child(
            &self,
            items: Vec<String>,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(items = "tail", verbatim = true)]
        fn leaf(&self, items: Vec<String>) -> Result<(), RemoteError>;
    }

    struct ExplicitInheritedTailSurrogateVerbatimSubtree;

    #[tool_definition]
    trait ExplicitInheritedTailSurrogateVerbatimParent {
        #[arg(count = "global")]
        fn explicit_inherited_tail_surrogate_verbatim_parent(
            &self,
            count: Vec<String>,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = ExplicitInheritedTailSurrogateVerbatimChild)]
        fn explicit_inherited_tail_surrogate_verbatim_child(
            &self,
        ) -> ExplicitInheritedTailSurrogateVerbatimSubtree;
    }

    #[test]
    fn promoted_explicit_inherited_tail_surrogate_rejects_verbatim_without_separator() {
        let result = __golem_tool_descriptor_for_ExplicitInheritedTailSurrogateVerbatimParent(
            &mut ToolBuildCtx::new(),
        );
        let err = match result {
            Ok(tool) => tool.try_to_tool().expect_err(
                "verbatim=true without a separator must remain invalid when an explicit inherited tail surrogate is promoted",
            ),
            Err(err) => err,
        };

        assert!(
            matches!(err, ToolBuildError::VerbatimWithoutSeparator(ref name) if name == "items"),
            "expected VerbatimWithoutSeparator for promoted explicit tail `items`, got {err:?}",
        );
    }

    #[tool_definition]
    trait AliasDeprojectedAncestorTailChild {
        #[arg(format = "global", aliases = ["count"])]
        fn alias_deprojected_ancestor_tail_child(
            &self,
            format: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        fn leaf(&self, items: Vec<String>, count: u32) -> Result<(), RemoteError>;
    }

    struct AliasDeprojectedAncestorTailSubtree;

    #[tool_definition]
    trait AliasDeprojectedAncestorTailParent {
        #[arg(count = "global")]
        fn alias_deprojected_ancestor_tail_parent(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = AliasDeprojectedAncestorTailChild)]
        fn alias_deprojected_ancestor_tail_child(&self) -> AliasDeprojectedAncestorTailSubtree;
    }

    #[test]
    fn deprojected_child_root_global_alias_keeps_strict_ancestor_for_leaf_tail_inference() {
        let tool = __golem_tool_descriptor_for_AliasDeprojectedAncestorTailParent(
            &mut ToolBuildCtx::new(),
        )
        .expect("descriptor builds");

        let child_idx = tool.commands[0]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| {
                tool.commands[idx as usize].name == "alias-deprojected-ancestor-tail-child"
            })
            .expect("child subtree is grafted") as usize;
        assert!(
            !tool.commands[child_idx]
                .globals
                .options
                .iter()
                .any(|o| o.long == "format"),
            "the child root global `format` is de-projected through alias `count` and must not survive",
        );

        let leaf_idx = tool.commands[child_idx]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| tool.commands[idx as usize].name == "leaf")
            .expect("leaf command is present") as usize;
        let body = tool.commands[leaf_idx].body.as_ref().expect("leaf body");

        assert!(
            !body.positionals.fixed.iter().any(|p| p.name == "count")
                && !body.options.iter().any(|o| o.long == "count"),
            "leaf-local `count` must be removed because strict ancestor global `count` remains effective",
        );
        assert_eq!(
            body.positionals.tail.as_ref().map(|t| t.name.as_str()),
            Some("items"),
            "the trailing strict-ancestor global re-declaration must not prevent `items` from being inferred as the tail positional",
        );
        assert!(
            !body.options.iter().any(|o| o.long == "items"),
            "`items` should not be projected as a repeatable-list option when the only later positional is de-projected",
        );
        tool.try_to_tool().expect("composed tool is valid");
    }

    #[tool_definition]
    trait PureStrictAncestorAliasTailChild {
        fn leaf(&self, items: Vec<String>, format: u32) -> Result<(), RemoteError>;
    }

    struct PureStrictAncestorAliasTailSubtree;

    #[tool_definition]
    trait PureStrictAncestorAliasTailParent {
        #[arg(count = "global", aliases = ["format"])]
        fn pure_strict_ancestor_alias_tail_parent(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = PureStrictAncestorAliasTailChild)]
        fn pure_strict_ancestor_alias_tail_child(&self) -> PureStrictAncestorAliasTailSubtree;
    }

    #[test]
    fn strict_ancestor_alias_deprojection_keeps_leaf_tail_inference_without_child_root_global() {
        let tool =
            __golem_tool_descriptor_for_PureStrictAncestorAliasTailParent(&mut ToolBuildCtx::new())
                .expect("descriptor builds");

        let child_idx = tool.commands[0]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| {
                tool.commands[idx as usize].name == "pure-strict-ancestor-alias-tail-child"
            })
            .expect("child subtree is grafted") as usize;
        let leaf_idx = tool.commands[child_idx]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| tool.commands[idx as usize].name == "leaf")
            .expect("leaf command is present") as usize;
        let body = tool.commands[leaf_idx].body.as_ref().expect("leaf body");

        assert!(
            !body.positionals.fixed.iter().any(|p| p.name == "format")
                && !body.options.iter().any(|o| o.long == "format"),
            "leaf-local `format` must be removed because strict ancestor global alias `format` remains effective",
        );
        assert_eq!(
            body.positionals.tail.as_ref().map(|t| t.name.as_str()),
            Some("items"),
            "the trailing strict-ancestor alias re-declaration must not prevent `items` from being inferred as the tail positional",
        );
        assert!(
            !body.options.iter().any(|o| o.long == "items"),
            "`items` should not be projected as a repeatable-list option when the only later positional is de-projected",
        );
        tool.try_to_tool().expect("composed tool is valid");
    }

    #[tool_definition]
    trait AliasDeprojectedOptionalPositionalChild {
        #[arg(format = "global", aliases = ["count"])]
        fn alias_deprojected_optional_positional_child(
            &self,
            format: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[arg(format = "positional", required = false)]
        fn leaf(&self, format: String, name: String) -> Result<(), RemoteError>;
    }

    struct AliasDeprojectedOptionalPositionalSubtree;

    #[tool_definition]
    trait AliasDeprojectedOptionalPositionalParent {
        #[arg(count = "global")]
        fn alias_deprojected_optional_positional_parent(
            &self,
            count: u32,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = AliasDeprojectedOptionalPositionalChild)]
        fn alias_deprojected_optional_positional_child(
            &self,
        ) -> AliasDeprojectedOptionalPositionalSubtree;
    }

    #[test]
    fn deprojected_child_root_global_alias_does_not_hide_optional_before_required() {
        let result = __golem_tool_descriptor_for_AliasDeprojectedOptionalPositionalParent(
            &mut ToolBuildCtx::new(),
        );
        let Ok(tool) = result else {
            return;
        };

        assert!(
            tool.try_to_tool().is_err(),
            "an optional fixed positional before a required fixed positional must be rejected either while building the descriptor or during runtime validation after de-projection leaves it local",
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

    #[test]
    fn inferred_tail_after_inherited_trailing_global_accepts_tail_separator() {
        let output = cargo_check_tool_crate(
            "inferred-tail-separator-after-inherited-global",
            r#"
use golem_rust::{tool_definition, ToolError};

#[derive(ToolError)]
enum E {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait GoodTool {
    #[arg(format = "global")]
    fn good_tool(&self, format: String, target: String) -> Result<(), E>;

    #[arg(items, separator = "--")]
    fn collect(&self, items: Vec<String>, format: String) -> Result<(), E>;
}
"#,
        );

        assert!(
            output.status.success(),
            "after inherited `format` is de-projected, `items` is the inferred tail; `separator` is a valid tail attr and should be consumed by the final tail shape, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn inferred_tail_after_inherited_trailing_global_accepts_tail_min() {
        let output = cargo_check_tool_crate(
            "inferred-tail-min-after-inherited-global",
            r#"
use golem_rust::{tool_definition, ToolError};

#[derive(ToolError)]
enum E {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait GoodTool {
    #[arg(format = "global")]
    fn good_tool(&self, format: String, target: String) -> Result<(), E>;

    #[arg(items, min = 1)]
    fn collect(&self, items: Vec<String>, format: String) -> Result<(), E>;
}
"#,
        );

        assert!(
            output.status.success(),
            "after inherited `format` is de-projected, `items` is the inferred tail; `min` is a valid tail occurrence bound and should be consumed by the final tail shape, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn non_tail_vec_option_signed_min_does_not_typecheck_unused_tail_form() {
        let output = cargo_check_tool_crate(
            "vec-option-signed-min-unused-tail",
            r#"
use golem_rust::{tool_definition, ToolError};

#[derive(ToolError)]
enum E {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait GoodTool {
    #[arg(nums, min = -5_i64)]
    fn run(&self, nums: Vec<i64>, suffix: String) -> Result<(), E>;
}
"#,
        );

        assert!(
            output.status.success(),
            "a non-tail Vec<i64> should treat min as an item numeric bound; the unused tail form must not typecheck it as u32\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn inferred_tail_custom_item_occurrence_min_does_not_typecheck_unused_option_form() {
        let output = cargo_check_tool_crate(
            "inferred-tail-custom-item-min-unused-option",
            r#"
use golem_rust::{tool_definition, FromSchema, IntoSchema, ToolError};

#[derive(Clone, IntoSchema, FromSchema)]
struct Item {
    value: String,
}

#[derive(ToolError)]
enum E {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait GoodTool {
    #[arg(items, min = 1)]
    fn run(&self, items: Vec<Item>) -> Result<(), E>;
}
"#,
        );

        assert!(
            output.status.success(),
            "an inferred tail over a custom item should treat min as an occurrence bound; the unused option form must not typecheck it as Item\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
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
        let output = cargo_test_tool_crate(
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
            "an explicit tail positional before a later fixed positional should be rejected, but cargo test succeeded\nstdout:\n{}\nstderr:\n{}",
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
    fn many_root_globals_with_leaf_command_compile() {
        let global_count = usize::BITS as usize;
        let mut source = String::from("use golem_rust::tool_definition;\n\n");
        source.push_str("#[tool_definition]\ntrait ManyGlobals {\n");
        for i in 0..global_count {
            source.push_str(&format!("    #[arg(g{i} = \"global\")]\n"));
        }
        source.push_str("    fn many_globals(\n        &self,\n");
        for i in 0..global_count {
            source.push_str(&format!("        g{i}: String,\n"));
        }
        source.push_str("        target: String,\n    );\n");
        source.push_str("    fn leaf(&self, value: String);\n}\n");

        let output = cargo_check_tool_crate("many-root-globals", &source);

        assert!(
            output.status.success(),
            "a tool definition should not panic or fail to compile merely because it has {global_count} root globals and a leaf command, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[tool_definition]
    trait ManyContextSensitiveLeafPositionals {
        fn leaf(
            &self,
            v0: Vec<String>,
            v1: Vec<String>,
            v2: Vec<String>,
            v3: Vec<String>,
            v4: Vec<String>,
            v5: Vec<String>,
            v6: Vec<String>,
            v7: Vec<String>,
            v8: Vec<String>,
            v9: Vec<String>,
            v10: Vec<String>,
            v11: Vec<String>,
            v12: Vec<String>,
        ) -> Result<(), RemoteError>;
    }

    #[test]
    fn many_context_sensitive_leaf_positionals_build_without_inherited_globals() {
        let tool = __golem_tool_descriptor_for_ManyContextSensitiveLeafPositionals(
            &mut ToolBuildCtx::new(),
        )
        .expect(
            "a standalone leaf with many Vec positionals and no inherited globals should use normal option/tail projection",
        );

        let leaf = tool
            .commands
            .iter()
            .find(|command| command.name == "leaf")
            .expect("leaf command exists");
        let body = leaf.body.as_ref().expect("leaf body");
        assert_eq!(
            body.positionals
                .tail
                .as_ref()
                .map(|tail| tail.name.as_str()),
            Some("v12"),
            "the final Vec parameter should infer as the tail positional",
        );
        for i in 0..12 {
            let name = format!("v{i}");
            assert!(
                body.options.iter().any(|option| {
                    option.long == name
                        && matches!(option.shape, ExtendedOptionShape::RepeatableList(_))
                }),
                "non-final Vec parameter {name} should project as a repeatable-list option",
            );
        }
        tool.try_to_tool().expect("standalone tool is valid");
    }

    #[test]
    fn many_context_sensitive_leaf_positionals_preserve_option_attrs_without_inherited_globals() {
        let output = cargo_check_tool_crate(
            "many-vec-option-attrs",
            r#"
use golem_rust::tool_definition;

#[tool_definition]
trait ManyVecOptionAttrs {
    #[arg(v0, short = 'a')]
    fn leaf(
        &self,
        v0: Vec<String>,
        v1: Vec<String>,
        v2: Vec<String>,
        v3: Vec<String>,
        v4: Vec<String>,
        v5: Vec<String>,
        v6: Vec<String>,
        v7: Vec<String>,
        v8: Vec<String>,
        v9: Vec<String>,
        v10: Vec<String>,
        v11: Vec<String>,
        v12: Vec<String>,
    );
}
"#,
        );

        assert!(
            output.status.success(),
            "with no inherited globals, only the final Vec is a tail; earlier Vec parameters are repeatable-list options and option attributes such as `short` must be accepted, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[tool_definition]
    trait SparseInheritedManyVecChild {
        fn leaf(
            &self,
            v0: Vec<String>,
            v1: Vec<String>,
            v2: Vec<String>,
            v3: Vec<String>,
            v4: Vec<String>,
            v5: Vec<String>,
            v6: Vec<String>,
            v7: Vec<String>,
            v8: Vec<String>,
            v9: Vec<String>,
            v10: Vec<String>,
            v11: Vec<String>,
            v12: Vec<String>,
        ) -> Result<(), RemoteError>;
    }

    struct SparseInheritedManyVecSubtree;

    #[tool_definition]
    trait SparseInheritedManyVecParent {
        #[arg(v0 = "global")]
        #[command(subtree = SparseInheritedManyVecChild)]
        fn sparse_inherited_many_vec_child(&self, v0: Vec<String>)
        -> SparseInheritedManyVecSubtree;
    }

    #[test]
    fn many_context_sensitive_leaf_positionals_allow_sparse_inherited_subset() {
        let tool =
            __golem_tool_descriptor_for_SparseInheritedManyVecParent(&mut ToolBuildCtx::new())
                .expect("only v0 is inherited; the remaining Vec parameters have a valid shape");

        let child_idx = tool.commands[0]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| tool.commands[idx as usize].name == "sparse-inherited-many-vec-child")
            .expect("child subtree is grafted") as usize;
        let leaf_idx = tool.commands[child_idx]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| tool.commands[idx as usize].name == "leaf")
            .expect("leaf command is present") as usize;
        let body = tool.commands[leaf_idx].body.as_ref().expect("leaf body");

        assert!(
            !body.options.iter().any(|option| option.long == "v0"),
            "v0 is de-projected onto the inherited global",
        );
        assert_eq!(
            body.positionals
                .tail
                .as_ref()
                .map(|tail| tail.name.as_str()),
            Some("v12"),
            "the final non-inherited Vec parameter remains the tail positional",
        );
        tool.try_to_tool().expect("composed tool is valid");
    }

    #[tool_definition]
    trait SparseInheritedTrailingManyVecChild {
        fn leaf(
            &self,
            v0: Vec<String>,
            v1: Vec<String>,
            v2: Vec<String>,
            v3: Vec<String>,
            v4: Vec<String>,
            v5: Vec<String>,
            v6: Vec<String>,
            v7: Vec<String>,
            v8: Vec<String>,
            v9: Vec<String>,
            v10: Vec<String>,
            v11: Vec<String>,
            v12: Vec<String>,
        ) -> Result<(), RemoteError>;
    }

    struct SparseInheritedTrailingManyVecSubtree;

    #[tool_definition]
    trait SparseInheritedTrailingManyVecParent {
        #[arg(v12 = "global")]
        #[command(subtree = SparseInheritedTrailingManyVecChild)]
        fn sparse_inherited_trailing_many_vec_child(
            &self,
            v12: Vec<String>,
        ) -> SparseInheritedTrailingManyVecSubtree;
    }

    #[test]
    fn many_context_sensitive_leaf_positionals_allow_sparse_trailing_inherited_subset() {
        let tool = __golem_tool_descriptor_for_SparseInheritedTrailingManyVecParent(
            &mut ToolBuildCtx::new(),
        )
        .expect("only v12 is inherited; v11 should become the remaining tail positional");

        let child_idx = tool.commands[0]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| {
                tool.commands[idx as usize].name == "sparse-inherited-trailing-many-vec-child"
            })
            .expect("child subtree is grafted") as usize;
        let leaf_idx = tool.commands[child_idx]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| tool.commands[idx as usize].name == "leaf")
            .expect("leaf command is present") as usize;
        let body = tool.commands[leaf_idx].body.as_ref().expect("leaf body");

        assert!(
            !body.options.iter().any(|option| option.long == "v12")
                && body
                    .positionals
                    .tail
                    .as_ref()
                    .map(|tail| tail.name.as_str())
                    != Some("v12"),
            "v12 is de-projected onto the inherited global",
        );
        assert_eq!(
            body.positionals
                .tail
                .as_ref()
                .map(|tail| tail.name.as_str()),
            Some("v11"),
            "after the trailing inherited Vec is de-projected, the preceding Vec parameter must be inferred as the tail positional",
        );
        tool.try_to_tool().expect("composed tool is valid");
    }

    #[tool_definition]
    trait MixedRootAndStrictSparseManyVecChild {
        #[arg(v0 = "global")]
        fn mixed_root_and_strict_sparse_many_vec_child(
            &self,
            v0: Vec<String>,
            target: String,
        ) -> Result<(), RemoteError>;

        fn leaf(
            &self,
            v0: Vec<String>,
            v1: Vec<String>,
            v2: Vec<String>,
            v3: Vec<String>,
            v4: Vec<String>,
            v5: Vec<String>,
            v6: Vec<String>,
            v7: Vec<String>,
            v8: Vec<String>,
            v9: Vec<String>,
            v10: Vec<String>,
            v11: Vec<String>,
            v12: Vec<String>,
        ) -> Result<(), RemoteError>;
    }

    struct MixedRootAndStrictSparseManyVecSubtree;

    #[tool_definition]
    trait MixedRootAndStrictSparseManyVecParent {
        #[arg(v12 = "global")]
        #[command(subtree = MixedRootAndStrictSparseManyVecChild)]
        fn mixed_root_and_strict_sparse_many_vec_child(
            &self,
            v12: Vec<String>,
        ) -> MixedRootAndStrictSparseManyVecSubtree;
    }

    #[test]
    fn many_context_sensitive_leaf_positionals_allow_mixed_root_and_sparse_strict_subset() {
        let tool = __golem_tool_descriptor_for_MixedRootAndStrictSparseManyVecParent(
            &mut ToolBuildCtx::new(),
        )
        .expect(
            "a bounded root global plus one sparse strict inherited global still leaves a valid leaf shape",
        );

        let child_idx = tool.commands[0]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| {
                tool.commands[idx as usize].name == "mixed-root-and-strict-sparse-many-vec-child"
            })
            .expect("child subtree is grafted") as usize;
        let leaf_idx = tool.commands[child_idx]
            .subcommands
            .iter()
            .copied()
            .find(|&idx| tool.commands[idx as usize].name == "leaf")
            .expect("leaf command is present") as usize;
        let body = tool.commands[leaf_idx].body.as_ref().expect("leaf body");

        assert!(
            !body.options.iter().any(|option| option.long == "v12")
                && body
                    .positionals
                    .tail
                    .as_ref()
                    .map(|tail| tail.name.as_str())
                    != Some("v12"),
            "v12 is de-projected onto the strict inherited global",
        );
        assert_eq!(
            body.positionals
                .tail
                .as_ref()
                .map(|tail| tail.name.as_str()),
            Some("v11"),
            "after the sparse strict inherited Vec is de-projected, v11 must be inferred as the tail positional even when another context-sensitive root global exists",
        );
        tool.try_to_tool().expect("composed tool is valid");
    }

    #[tool_definition]
    trait ManyOverlappingGraftedRootGlobalsChild {
        #[arg(g0 = "global")]
        #[arg(g1 = "global")]
        #[arg(g2 = "global")]
        #[arg(g3 = "global")]
        #[arg(g4 = "global")]
        #[arg(g5 = "global")]
        #[arg(g6 = "global")]
        #[arg(g7 = "global")]
        #[arg(g8 = "global")]
        #[arg(g9 = "global")]
        #[arg(g10 = "global")]
        #[arg(g11 = "global")]
        #[arg(g12 = "global")]
        fn many_overlapping_grafted_root_globals_child(
            &self,
            g0: String,
            g1: String,
            g2: String,
            g3: String,
            g4: String,
            g5: String,
            g6: String,
            g7: String,
            g8: String,
            g9: String,
            g10: String,
            g11: String,
            g12: String,
            target: String,
        ) -> Result<(), RemoteError>;

        fn leaf(
            &self,
            g0: String,
            g1: String,
            g2: String,
            g3: String,
            g4: String,
            g5: String,
            g6: String,
            g7: String,
            g8: String,
            g9: String,
            g10: String,
            g11: String,
            g12: String,
            value: String,
        ) -> Result<(), RemoteError>;
    }

    struct ManyOverlappingGraftedRootGlobalsSubtree;

    #[tool_definition]
    trait ManyOverlappingGraftedRootGlobalsParent {
        #[arg(g0 = "global")]
        fn many_overlapping_grafted_root_globals_parent(
            &self,
            g0: String,
            target: String,
        ) -> Result<(), RemoteError>;

        #[command(subtree = ManyOverlappingGraftedRootGlobalsChild)]
        fn many_overlapping_grafted_root_globals_child(
            &self,
        ) -> ManyOverlappingGraftedRootGlobalsSubtree;
    }

    #[test]
    fn many_overlapping_grafted_root_globals_build_when_one_is_deprojected() {
        let tool = __golem_tool_descriptor_for_ManyOverlappingGraftedRootGlobalsParent(
            &mut ToolBuildCtx::new(),
        )
        .expect(
            "a valid grafted child with many overlapping root globals should build when one child root global is de-projected",
        );

        let child = tool
            .commands
            .iter()
            .find(|command| command.name == "many-overlapping-grafted-root-globals-child")
            .expect("child subtree root is grafted");
        assert!(
            !child
                .globals
                .options
                .iter()
                .any(|option| option.long == "g0"),
            "the child root's duplicate g0 global is de-projected onto the parent root global"
        );
        tool.try_to_tool().expect("composed tool is valid");
    }

    #[test]
    fn optional_fixed_positional_before_required_fixed_positional_is_rejected() {
        let output = cargo_test_tool_crate(
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
            "an optional fixed positional before a required fixed positional should be rejected, but cargo test succeeded\nstdout:\n{}\nstderr:\n{}",
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
    fn bug_finder_auto_injected_principal_rejects_arg_attributes_that_would_be_ignored() {
        let output = cargo_check_tool_crate(
            "principal-arg-attr",
            r#"
use golem_rust::{tool_definition, tool_implementation};

#[tool_definition]
trait BadTool {
    #[arg(principal = "option")]
    fn run(&self, principal: golem_rust::agentic::Principal, name: String) -> String;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _principal: golem_rust::agentic::Principal, name: String) -> String {
        name
    }
}
"#,
        );

        assert!(
            !output.status.success(),
            "an auto-injected Principal is projected only from its exact SDK/WIT type and any `#[arg]` field would be silently dropped, so it must be rejected, but cargo check succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn tool_client_accepts_public_wasip2_stdin_stream() {
        let output = cargo_check_tool_crate(
            "tool-client-wasip2-stdin",
            r#"
use golem_rust::agentic::invoke_and_await_infallible;
use golem_rust::bindings::golem::tool::host::ToolRpc;
use golem_rust::wasip2::io::streams::InputStream;

fn forward_stdin(rpc: &ToolRpc, input: &golem_rust::TypedSchemaValue, stdin: InputStream) {
    let _ = invoke_and_await_infallible(rpc, &[], input, Some(stdin));
}
"#,
        );

        assert!(
            output.status.success(),
            "tool client helpers must accept the same public wasip2 stream type that tool definitions use, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn tool_client_allows_command_named_new() {
        let output = cargo_check_tool_crate(
            "tool-client-command-named-new",
            r#"
use golem_rust::tool_definition;

#[tool_definition]
trait Project {
    fn new(&self, name: String) -> String;
}

fn build_client() {
    let client = ProjectClient::default();
    let _future = client.new("demo".to_string());
}
"#,
        );

        assert!(
            output.status.success(),
            "a valid tool command named `new` must not collide with the generated client constructor, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn bug_finder_command_named_new_does_not_reserve_fallback_constructor_name() {
        let output = cargo_check_tool_crate(
            "tool-client-new-fallback-name-collision",
            r#"
use golem_rust::tool_definition;

#[tool_definition]
trait Project {
    fn new(&self, name: String) -> String;
    fn __golem_tool_client_new(&self, name: String) -> String;
}

fn build_client() {
    let client = ProjectClient::default();
    let _first = client.new("demo".to_string());
    let _second = client.__golem_tool_client_new("demo".to_string());
}
"#,
        );

        assert!(
            output.status.success(),
            "renaming the generated constructor away from `new` must not reserve a valid command method name; cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn bug_finder_command_named_with_parts_does_not_collide_with_hidden_client_helper() {
        let output = cargo_check_tool_crate(
            "tool-client-with-parts-name-collision",
            r#"
use golem_rust::tool_definition;

#[tool_definition]
trait Project {
    fn __golem_tool_client_with_parts(&self, name: String) -> String;
}

fn build_client() {
    let client = ProjectClient::default();
    let _future = client.__golem_tool_client_with_parts("demo".to_string());
}
"#,
        );

        assert!(
            output.status.success(),
            "valid command names must not collide with generated client helpers, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn subtree_placeholder_return_type_does_not_need_magic_suffix() {
        let output = cargo_check_tool_crate(
            "subtree-placeholder-return-type",
            r#"
use golem_rust::{tool_definition, tool_implementation};

#[tool_definition]
trait ChildTool {
    fn leaf(&self) -> String;
}

struct ChildHandle;

#[tool_definition]
trait ParentTool {
    #[command(subtree = ChildTool)]
    fn child_tool(&self) -> ChildHandle;
}

struct ParentToolImpl;

#[tool_implementation]
impl ParentTool for ParentToolImpl {
    fn child_tool(&self) -> ChildHandle {
        ChildHandle
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "the #[command(subtree = ...)] attribute, not a return-type suffix, marks a subtree dispatcher placeholder, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn tool_implementation_on_named_field_struct_still_compiles() {
        let output = cargo_check_tool_crate(
            "stateful-tool-implementation",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum E {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait StatefulTool {
    fn run(&self, input: String) -> Result<String, E>;
}

struct StatefulToolImpl {
    prefix: String,
}

#[tool_implementation]
impl StatefulTool for StatefulToolImpl {
    fn run(&self, input: String) -> Result<String, E> {
        Ok(format!("{}{}", self.prefix, input))
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "#[tool_implementation] is applied to trait impls and should not impose an undocumented unit-struct-only restriction, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn tool_implementation_allows_wildcard_impl_parameter_names() {
        let output = cargo_check_tool_crate(
            "wildcard-impl-parameter",
            r#"
use golem_rust::{tool_definition, tool_implementation, ToolError};

#[derive(ToolError)]
enum E {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Bad(String),
}

#[tool_definition]
trait UnusedParamTool {
    fn run(&self, input: String) -> Result<String, E>;
}

struct UnusedParamToolImpl;

#[tool_implementation]
impl UnusedParamTool for UnusedParamToolImpl {
    fn run(&self, _: String) -> Result<String, E> {
        Ok("ok".to_string())
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "#[tool_implementation] should accept ordinary Rust impl parameter patterns such as `_`, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn user_value_type_named_principal_is_not_auto_injected() {
        let output = cargo_check_tool_crate(
            "user-principal-value-type",
            r#"
use golem_rust::{tool_definition, tool_implementation, FromSchema, IntoSchema};

mod domain {
    use super::*;

    #[derive(IntoSchema, FromSchema)]
    pub struct Principal {
        pub id: String,
    }
}

#[tool_definition]
trait DomainTool {
    fn run(&self, principal: domain::Principal) -> String;
}

struct DomainToolImpl;

#[tool_implementation]
impl DomainTool for DomainToolImpl {
    fn run(&self, principal: domain::Principal) -> String {
        principal.id
    }
}

fn build_client() {
    let client = DomainToolClient::default();
    let _future = client.run(domain::Principal { id: "u".to_string() });
}
"#,
        );

        assert!(
            output.status.success(),
            "a user-defined value type named `Principal` should be treated by path as a normal schema value, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn bug_finder_domain_principal_parameter_is_schema_input() {
        let output = cargo_check_tool_crate(
            "bug-finder-domain-principal-input",
            r#"
use golem_rust::{tool_definition, tool_implementation, FromSchema, IntoSchema};

mod domain {
    use super::*;

    #[derive(IntoSchema, FromSchema)]
    pub struct Principal {
        pub id: String,
    }
}

#[tool_definition]
trait DomainPrincipalTool {
    fn lookup(&self, principal: domain::Principal) -> String;
}

struct DomainPrincipalToolImpl;

#[tool_implementation]
impl DomainPrincipalTool for DomainPrincipalToolImpl {
    fn lookup(&self, principal: domain::Principal) -> String {
        principal.id
    }
}

fn compile_client_and_impl() {
    let client = DomainPrincipalToolClient::default();
    let _future = client.lookup(domain::Principal { id: "user-1".to_string() });
}
"#,
        );

        assert!(
            output.status.success(),
            "a user-defined schema type whose last path segment is `Principal` must not be treated as the host principal; cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn bug_finder_local_principal_type_is_schema_input() {
        let output = cargo_check_tool_crate(
            "bug-finder-local-principal-input",
            r#"
use golem_rust::{tool_definition, tool_implementation, FromSchema, IntoSchema};

#[derive(IntoSchema, FromSchema)]
pub struct Principal {
    pub id: String,
}

#[tool_definition]
trait LocalPrincipalTool {
    fn lookup(&self, principal: Principal) -> String;
}

struct LocalPrincipalToolImpl;

#[tool_implementation]
impl LocalPrincipalTool for LocalPrincipalToolImpl {
    fn lookup(&self, principal: Principal) -> String {
        principal.id
    }
}

fn compile_client_and_impl() {
    let client = LocalPrincipalToolClient::default();
    let _future = client.lookup(Principal { id: "user-1".to_string() });
}
"#,
        );

        assert!(
            output.status.success(),
            "a local user-defined schema type named `Principal` should be treated as a normal schema value, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn agentic_world_tool_rpc_is_accepted_by_tool_client_helpers() {
        let output = cargo_check_tool_crate(
            "agentic-world-tool-rpc-client-helper",
            r#"
use golem_rust::agentic::invoke_and_await_infallible;
use golem_rust::golem_agentic::golem::tool::host::ToolRpc;

fn call_tool(rpc: &ToolRpc, input: &golem_rust::TypedSchemaValue) {
    let _ = invoke_and_await_infallible(rpc, &[], input, None);
}
"#,
        );

        assert!(
            output.status.success(),
            "tool client helpers must accept the ToolRpc type exposed by the exported agentic world, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn exported_agentic_world_uses_public_wasip2_streams() {
        let output = cargo_check_tool_crate(
            "agentic-world-wasip2-streams",
            r#"
use golem_rust::golem_agentic::exports::golem::tool::guest::{Guest, InvocationResult, Tool, ToolError, TypedSchemaValue};
use golem_rust::golem_agentic::golem::agent::common::Principal;
use golem_rust::wasip2::io::streams::InputStream;

struct Component;

impl Guest for Component {
    fn discover_tools() -> Result<Vec<Tool>, ToolError> {
        unimplemented!()
    }

    fn get_tool(_name: String) -> Result<Tool, ToolError> {
        unimplemented!()
    }

    fn invoke(
        _tool_name: String,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<InputStream>,
        _principal: Principal,
    ) -> Result<InvocationResult, ToolError> {
        unimplemented!()
    }
}
"#,
        );

        assert!(
            output.status.success(),
            "the exported agentic world should expose the same public wasip2 stream types as tool definitions and tool clients, but cargo check failed\nstdout:\n{}\nstderr:\n{}",
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
        cargo_tool_crate(name, source, "check")
    }

    fn cargo_test_tool_crate(name: &str, source: &str) -> std::process::Output {
        cargo_tool_crate(name, source, "test")
    }

    fn cargo_tool_crate(name: &str, source: &str, command: &str) -> std::process::Output {
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
            .arg(command)
            .arg("--quiet")
            .env("CARGO_TARGET_DIR", target_dir)
            .current_dir(&root)
            .output()
            .unwrap_or_else(|error| {
                panic!("failed to run cargo {command} for temporary tool crate: {error}")
            });

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
