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

use super::*;

impl ToolSchemaChecks {
    pub fn dynamic() -> Self {
        Self(check_graph_closed)
    }

    pub fn scalar() -> Self {
        Self(check_scalar_graph)
    }
}

impl<T> SelectToolSchemaChecks for &&ToolSchemaProbe<T> {
    fn tool_schema_checks(self) -> Option<ToolSchemaChecks> {
        Some(ToolSchemaChecks::dynamic())
    }

    fn tool_value_schema(
        self,
        fallback: impl FnOnce() -> Result<SchemaGraph, ToolBuildError>,
    ) -> Result<SchemaGraph, ToolBuildError> {
        fallback()
    }
}

macro_rules! scalar_schema_checks {
    ($($ty:ty => $constructor:ident),* $(,)?) => {$(
        impl SelectToolSchemaChecks for &ToolSchemaProbe<$ty> {
            fn tool_schema_checks(self) -> Option<ToolSchemaChecks> { None }

            #[inline]
            fn tool_value_schema(
                self,
                _fallback: impl FnOnce() -> Result<SchemaGraph, ToolBuildError>,
            ) -> Result<SchemaGraph, ToolBuildError> {
                Ok(SchemaGraph::anonymous(SchemaType::$constructor()))
            }
        }
    )*};
}

scalar_schema_checks!(
    bool => bool, u8 => u8, u16 => u16, u32 => u32, u64 => u64,
    i8 => s8, i16 => s16, i32 => s32, i64 => s64,
    f32 => f32, f64 => f64, char => char, String => string,
);

pub(super) fn check_scalar_graph(
    graph: &SchemaGraph,
    position: &str,
) -> Result<(), ToolBuildError> {
    let scalar = matches!(
        graph.root,
        SchemaType::Bool { .. }
            | SchemaType::String { .. }
            | SchemaType::Char { .. }
            | SchemaType::U8 {
                restrictions: None,
                ..
            }
            | SchemaType::U16 {
                restrictions: None,
                ..
            }
            | SchemaType::U32 {
                restrictions: None,
                ..
            }
            | SchemaType::U64 {
                restrictions: None,
                ..
            }
            | SchemaType::S8 {
                restrictions: None,
                ..
            }
            | SchemaType::S16 {
                restrictions: None,
                ..
            }
            | SchemaType::S32 {
                restrictions: None,
                ..
            }
            | SchemaType::S64 {
                restrictions: None,
                ..
            }
            | SchemaType::F32 {
                restrictions: None,
                ..
            }
            | SchemaType::F64 {
                restrictions: None,
                ..
            }
    );
    if scalar && graph.defs.is_empty() {
        Ok(())
    } else {
        Err(ToolBuildError::IllFormedSchema {
            position: position.to_string(),
            detail: "expected an unrefined scalar schema".to_string(),
        })
    }
}

/// The `value-is` comparand for an option, keyed by whether the option collects
/// occurrences. A scalar / optional-scalar option is non-collecting: its
/// declared value graph is matched with the whole-or-one-peel relaxation. A
/// repeatable-list option collects into a list, so its comparand is the
/// per-occurrence item type matched exactly; a repeatable-map option collects
/// into a map, so its comparand is the per-entry map *value* type matched
/// exactly. A repeatable-map whose map type does not resolve to a map yields
/// [`ValueComparand::BlockedByTypeError`] (the malformed type is reported by
/// [`validate_tool`]). Definition graphs are preserved so any `Ref` still
/// resolves.
fn option_value_is_comparand<S: ToolSchemaRepr>(
    shape: &ExtendedOptionShape<S>,
) -> ValueComparand<S> {
    match shape {
        ExtendedOptionShape::Scalar(g) | ExtendedOptionShape::OptionalScalar(g) => {
            ValueComparand::Type(ValueIsComparand {
                graph: g.clone(),
                mode: ValueIsMode::WholeOrOnePeel,
            })
        }
        ExtendedOptionShape::RepeatableList(r) => ValueComparand::Type(ValueIsComparand {
            graph: r.item_type.clone(),
            mode: ValueIsMode::Exact,
        }),
        ExtendedOptionShape::RepeatableMap(r) if r.map_type.map_value().is_some() => {
            ValueComparand::Type(ValueIsComparand {
                graph: r.map_type.map_value().unwrap(),
                mode: ValueIsMode::Exact,
            })
        }
        ExtendedOptionShape::RepeatableMap(_) => ValueComparand::BlockedByTypeError,
    }
}

/// The authored, self-contained [`SchemaGraph`] backing an option's value
/// type, regardless of how the option collects on the command line.
fn option_authored_graph(shape: &ExtendedOptionShape) -> &SchemaGraph {
    match shape {
        ExtendedOptionShape::Scalar(g) | ExtendedOptionShape::OptionalScalar(g) => g,
        ExtendedOptionShape::RepeatableList(s) => &s.item_type,
        ExtendedOptionShape::RepeatableMap(s) => &s.map_type,
    }
}

/// Validate a self-contained per-argument [`SchemaGraph`] for structural
/// well-formedness via [`validate_graph`]: every embedded type (root and every
/// definition body) is well-formed, every [`SchemaType::Ref`] resolves within
/// the graph's own `defs`, and inline restrictions (numeric bounds, text/binary
/// ranges, union discriminators, ...) are valid. A dangling reference is
/// surfaced as a position-aware [`ToolBuildError::UnresolvedTypeRef`]; any other
/// well-formedness failure becomes [`ToolBuildError::IllFormedSchema`].
///
/// Closedness (no dangling refs) is also what makes per-argument validation
/// equivalent to validation against the merged tool schema: [`merge_agent_graphs`]
/// only unions defs (rejecting id collisions with conflicting bodies), so once
/// each embedded graph is well-formed and closed, resolving a ref / validating a
/// default or `value-is` literal against the local graph yields the same result
/// as against the merged graph.
pub(super) fn check_graph_closed(
    graph: &SchemaGraph,
    position: &str,
) -> Result<(), ToolBuildError> {
    let errors = match golem_schema::schema::validation::validate_graph(graph) {
        Ok(()) => return Ok(()),
        Err(errors) => errors,
    };
    // Report the first error deterministically (validate_graph collects in a
    // stable discovery order), preferring the precise dangling-ref variant.
    let first = errors
        .iter()
        .find(|e| {
            matches!(
                e,
                golem_schema::schema::validation::SchemaError::DanglingRef(_)
            )
        })
        .or_else(|| errors.first());
    match first {
        Some(golem_schema::schema::validation::SchemaError::DanglingRef(id)) => {
            Err(ToolBuildError::UnresolvedTypeRef {
                position: position.to_string(),
                id: id.to_string(),
            })
        }
        Some(other) => Err(ToolBuildError::IllFormedSchema {
            position: position.to_string(),
            detail: other.to_string(),
        }),
        None => Ok(()),
    }
}

fn validate_default(value: &SchemaValue, graph: &SchemaGraph) -> Result<(), ToolBuildError> {
    validate_value(graph, &graph.root, value)
        .map_err(|e| ToolBuildError::DefaultTypeMismatch(format!("{e:?}")))
}

/// Whether a `value-is` literal is compatible with its [`ValueIsComparand`].
///
/// The literal is always compatible if it is a valid value for the comparand
/// graph as a whole. For a [`ValueIsMode::WholeOrOnePeel`] comparand (a
/// non-collecting value surface) it is *also* compatible — under the WIT "any
/// element / entry equals this literal" relaxation — if it is a valid value for
/// the element type of a list-shaped, or the value type of a map-shaped,
/// (optionally `option`-wrapped) comparand. A [`ValueIsMode::Exact`] comparand
/// (a collecting surface, whose graph is already the per-occurrence type) gets
/// no relaxation: matching one more level would descend past a single
/// occurrence (e.g. accept `1` against a `Vec<Vec<u32>>` option whose occurrence
/// is `list<u32>`).
pub(super) fn value_is_compatible(comparand: &ValueIsComparand, value: &SchemaValue) -> bool {
    let graph = &comparand.graph;
    if validate_value(graph, &graph.root, value).is_ok() {
        return true;
    }
    if comparand.mode == ValueIsMode::Exact {
        return false;
    }
    let Ok(mut peeled) = graph.resolve_ref(&graph.root) else {
        return false;
    };
    while let SchemaType::Option { inner, .. } = peeled {
        match graph.resolve_ref(inner) {
            Ok(next) => peeled = next,
            Err(_) => return false,
        }
    }
    match peeled {
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            validate_value(graph, element, value).is_ok()
        }
        SchemaType::Map {
            value: map_value, ..
        } => validate_value(graph, map_value, value).is_ok(),
        _ => false,
    }
}

/// `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`: lowercase kebab-case starting with a letter,
/// no leading/trailing or doubled dashes. Hand-rolled to avoid a `regex`
/// dependency in the guest SDK.
fn is_valid_identifier(s: &str) -> bool {
    let mut prev_dash = false;
    !s.is_empty()
        && s.chars().enumerate().all(|(i, c)| {
            let ok = if i == 0 {
                c.is_ascii_lowercase()
            } else {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
            };
            let dash_ok = !(c == '-' && prev_dash);
            prev_dash = c == '-';
            ok && dash_ok
        })
        && !s.ends_with('-')
}

fn check_identifier(kind: &'static str, value: &str) -> Result<(), ToolBuildError> {
    if is_valid_identifier(value) {
        Ok(())
    } else {
        Err(ToolBuildError::InvalidIdentifier {
            kind,
            value: value.to_string(),
        })
    }
}

fn insert_unique(set: &mut BTreeSet<String>, name: &str) -> Result<(), ToolBuildError> {
    if set.insert(name.to_string()) {
        Ok(())
    } else {
        Err(ToolBuildError::DuplicateName(name.to_string()))
    }
}

fn insert_unique_short(set: &mut BTreeSet<char>, short: char) -> Result<(), ToolBuildError> {
    if set.insert(short) {
        Ok(())
    } else {
        Err(ToolBuildError::DuplicateShort(short))
    }
}

/// Validate a [`Tool`] against the producer-side construction invariants
/// documented in `golem:tool/common`. Mirrors the canonical host-side
/// validator (`golem-common`'s `validate_tool`), adapted to the SDK's
/// per-argument [`SchemaGraph`] representation. Type/value well-formedness of
/// embedded schemas is delegated to [`validate_value`]; each embedded graph is
/// validated for structural well-formedness — including dangling
/// [`SchemaType::Ref`]s and ill-formed inline restrictions (e.g. numeric
/// `min > max`) — by [`check_graph_closed`] (the closedness that also makes
/// per-argument validation equivalent to validating against the merged tool
/// schema). Cross-graph id collisions with conflicting bodies surface later
/// during graph merge in [`ExtendedToolType::try_to_tool`].
///
/// Runs the structural command-tree check first so the subsequent recursive
/// traversal can index the tree without bounds/cycle hazards.
pub fn validate_tool(tool: &ExtendedToolType) -> Result<(), ToolBuildError> {
    validate_tool_with::<true>(tool, check_graph_closed)
}

pub(super) fn validate_tool_with<const VALUES: bool>(
    tool: &ExtendedToolType,
    graph_check: ToolGraphCheck,
) -> Result<(), ToolBuildError> {
    check_command_tree_structure(tool)?;
    visit_command::<VALUES>(tool, 0, &[], graph_check)
}

/// Structural integrity of the command tree: non-empty, every subcommand index
/// in bounds, acyclic, single-rooted tree (no shared subcommands), all nodes
/// reachable from the root.
pub(super) fn check_command_tree_structure(tool: &ExtendedToolType) -> Result<(), ToolBuildError> {
    let len = tool.commands.len();
    if len == 0 {
        return Err(ToolBuildError::EmptyCommandTree);
    }
    for node in &tool.commands {
        for sub in &node.subcommands {
            if *sub < 0 || (*sub as usize) >= len {
                return Err(ToolBuildError::CommandIndexOutOfBounds { index: *sub, len });
            }
        }
    }
    let mut visited = vec![false; len];
    let mut on_stack = vec![false; len];
    dfs_command_tree(tool, 0, &mut visited, &mut on_stack)?;
    for (i, seen) in visited.iter().enumerate() {
        if !seen {
            return Err(ToolBuildError::UnreachableCommandNode(i as i32));
        }
    }
    Ok(())
}

fn dfs_command_tree(
    tool: &ExtendedToolType,
    idx: usize,
    visited: &mut [bool],
    on_stack: &mut [bool],
) -> Result<(), ToolBuildError> {
    if on_stack[idx] {
        return Err(ToolBuildError::CommandTreeCycle(idx as i32));
    }
    if visited[idx] {
        return Err(ToolBuildError::DuplicateCommandParent(idx as i32));
    }
    visited[idx] = true;
    on_stack[idx] = true;
    for sub in &tool.commands[idx].subcommands {
        // Bounds were validated in check_command_tree_structure.
        dfs_command_tree(tool, *sub as usize, visited, on_stack)?;
    }
    on_stack[idx] = false;
    Ok(())
}

/// Recursive, scope-aware traversal mirroring the canonical validator.
/// `ancestor_globals` are the globals of strict ancestors (root excluded only
/// at the root call); the current node's own globals are appended to form the
/// in-scope set for its body and children.
fn visit_command<const VALUES: bool>(
    tool: &ExtendedToolType,
    index: usize,
    ancestor_globals: &[&ExtendedGlobals],
    graph_check: ToolGraphCheck,
) -> Result<(), ToolBuildError> {
    let node = &tool.commands[index];
    check_identifier("command name", &node.name)?;
    for alias in &node.aliases {
        check_identifier("command alias", alias)?;
    }
    check_globals_decls::<VALUES>(&node.globals, graph_check)?;
    check_global_scope_uniqueness(ancestor_globals, &node.globals)?;

    let mut in_scope: Vec<&ExtendedGlobals> = ancestor_globals.to_vec();
    in_scope.push(&node.globals);

    if let Some(body) = &node.body {
        check_body::<VALUES>(body, &in_scope, graph_check)?;
    }
    check_subcommand_uniqueness(tool, node)?;

    for sub in &node.subcommands {
        visit_command::<VALUES>(tool, *sub as usize, &in_scope, graph_check)?;
    }
    Ok(())
}

/// Identifier, repeatable-map, default, and variant-in-input checks for the
/// declarations within one command's `globals` (uniqueness is handled by
/// [`check_global_scope_uniqueness`]).
fn check_globals_decls<const VALUES: bool>(
    globals: &ExtendedGlobals,
    graph_check: ToolGraphCheck,
) -> Result<(), ToolBuildError> {
    for opt in &globals.options {
        check_option_decl::<VALUES>(opt, graph_check)?;
    }
    for flag in &globals.flags {
        check_flag_identifiers(flag)?;
    }
    Ok(())
}

fn check_option_decl<const VALUES: bool>(
    opt: &ExtendedOptionSpec,
    graph_check: ToolGraphCheck,
) -> Result<(), ToolBuildError> {
    check_identifier("option long name", &opt.long)?;
    for alias in &opt.aliases {
        check_identifier("option alias", alias)?;
    }
    graph_check(
        option_authored_graph(&opt.shape),
        &format!("option --{}", opt.long),
    )?;
    if let ExtendedOptionShape::RepeatableMap(shape) = &opt.shape
        && !resolves_to_map(&shape.map_type)
    {
        return Err(ToolBuildError::RepeatableMapTypeNotMap(opt.long.clone()));
    }
    if let Some(default) = &opt.default {
        if !VALUES {
            return Err(ToolBuildError::DefaultTypeMismatch(
                "literal-free descriptor contains a default".to_string(),
            ));
        }
        validate_default(default, &option_collected_graph(&opt.shape))?;
    }
    let input = option_input_graph(&opt.shape);
    if graph_reaches_variant(&input) {
        return Err(ToolBuildError::VariantInInputPosition(opt.long.clone()));
    }
    Ok(())
}

fn check_flag_identifiers(flag: &FlagSpec) -> Result<(), ToolBuildError> {
    check_identifier("flag long name", &flag.long)?;
    for alias in &flag.aliases {
        check_identifier("flag alias", alias)?;
    }
    Ok(())
}

/// The current command's own globals must be unique among themselves and
/// against every ancestor global (long names, aliases, and short forms).
fn check_global_scope_uniqueness(
    ancestors: &[&ExtendedGlobals],
    own: &ExtendedGlobals,
) -> Result<(), ToolBuildError> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut shorts: BTreeSet<char> = BTreeSet::new();
    for globals in ancestors {
        seed_global_tokens(globals, &mut names, &mut shorts);
    }
    for opt in &own.options {
        insert_unique(&mut names, &opt.long)?;
        for alias in &opt.aliases {
            insert_unique(&mut names, alias)?;
        }
        if let Some(short) = opt.short {
            insert_unique_short(&mut shorts, short)?;
        }
    }
    for flag in &own.flags {
        insert_unique(&mut names, &flag.long)?;
        for alias in &flag.aliases {
            insert_unique(&mut names, alias)?;
        }
        if let Some(short) = flag.short {
            insert_unique_short(&mut shorts, short)?;
        }
    }
    Ok(())
}

fn seed_global_tokens(
    globals: &ExtendedGlobals,
    names: &mut BTreeSet<String>,
    shorts: &mut BTreeSet<char>,
) {
    for opt in &globals.options {
        names.insert(opt.long.clone());
        names.extend(opt.aliases.iter().cloned());
        if let Some(short) = opt.short {
            shorts.insert(short);
        }
    }
    for flag in &globals.flags {
        names.insert(flag.long.clone());
        names.extend(flag.aliases.iter().cloned());
        if let Some(short) = flag.short {
            shorts.insert(short);
        }
    }
}

/// Registers a flag's referenceable names. A flag carries no value type, so it
/// is never added to [`NameScope::typed`]; a `value-is` against it is rejected.
pub(super) fn register_flag_scope<S: ToolSchemaRepr>(scope: &mut NameScope<S>, flag: &FlagSpec) {
    scope.names.insert(flag.long.clone());
    scope.names.extend(flag.aliases.iter().cloned());
}

/// Registers a fixed positional's name and its whole-or-one-peel comparand (a
/// fixed positional is a non-collecting value surface: its declared type is the
/// comparand).
pub(super) fn register_fixed_positional_scope<S: ToolSchemaRepr>(
    scope: &mut NameScope<S>,
    positional: &ExtendedPositional<S>,
) {
    scope.names.insert(positional.name.clone());
    scope.typed.insert(
        positional.name.clone(),
        ValueComparand::Type(ValueIsComparand {
            graph: positional.type_.clone(),
            mode: ValueIsMode::WholeOrOnePeel,
        }),
    );
}

/// Registers a tail positional's name and its per-occurrence comparand. A tail
/// collects occurrences into a `list<item>`, so a `value-is` literal matches one
/// item exactly (the tail's `item_type`), never the whole collected list.
pub(super) fn register_tail_scope<S: ToolSchemaRepr>(
    scope: &mut NameScope<S>,
    tail: &ExtendedTailPositional<S>,
) {
    scope.names.insert(tail.name.clone());
    scope.typed.insert(
        tail.name.clone(),
        ValueComparand::Type(ValueIsComparand {
            graph: tail.item_type.clone(),
            mode: ValueIsMode::Exact,
        }),
    );
}

fn check_body<const VALUES: bool>(
    body: &ExtendedCommandBody,
    in_scope: &[&ExtendedGlobals],
    graph_check: ToolGraphCheck,
) -> Result<(), ToolBuildError> {
    // In-scope global tokens (for uniqueness) and resolution scope (for refs).
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut shorts: BTreeSet<char> = BTreeSet::new();
    let mut scope = NameScope::default();
    for globals in in_scope {
        seed_global_tokens(globals, &mut names, &mut shorts);
        for opt in &globals.options {
            register_option_scope(&mut scope, opt);
        }
        for flag in &globals.flags {
            register_flag_scope(&mut scope, flag);
        }
    }

    for opt in &body.options {
        check_option_decl::<VALUES>(opt, graph_check)?;
        insert_unique(&mut names, &opt.long)?;
        for alias in &opt.aliases {
            insert_unique(&mut names, alias)?;
        }
        if let Some(short) = opt.short {
            insert_unique_short(&mut shorts, short)?;
        }
        register_option_scope(&mut scope, opt);
    }

    for flag in &body.flags {
        check_flag_identifiers(flag)?;
        insert_unique(&mut names, &flag.long)?;
        for alias in &flag.aliases {
            insert_unique(&mut names, alias)?;
        }
        if let Some(short) = flag.short {
            insert_unique_short(&mut shorts, short)?;
        }
        register_flag_scope(&mut scope, flag);
    }

    // Optional fixed positionals must be trailing: once an optional one appears,
    // no required one may follow, or the boundary between them is ambiguous. The
    // macro enforces this for locally declared positionals, but inherited-global
    // de-projection can leave a re-declared optional positional local at runtime
    // (its inherited global is itself de-projected against a strict ancestor), so
    // the surviving order is re-checked here.
    let mut saw_optional_positional = false;
    for positional in &body.positionals.fixed {
        check_identifier("positional name", &positional.name)?;
        graph_check(
            &positional.type_,
            &format!("positional {}", positional.name),
        )?;
        insert_unique(&mut names, &positional.name)?;
        register_fixed_positional_scope(&mut scope, positional);
        if let Some(default) = &positional.default {
            if !VALUES {
                return Err(ToolBuildError::DefaultTypeMismatch(
                    "literal-free descriptor contains a default".to_string(),
                ));
            }
            validate_default(default, &positional.type_)?;
        }
        if graph_reaches_variant(&positional.type_) {
            return Err(ToolBuildError::VariantInInputPosition(
                positional.name.clone(),
            ));
        }
        if positional.required {
            if saw_optional_positional {
                return Err(ToolBuildError::RequiredPositionalAfterOptional(
                    positional.name.clone(),
                ));
            }
        } else {
            saw_optional_positional = true;
        }
    }

    if let Some(tail) = &body.positionals.tail {
        check_identifier("positional name", &tail.name)?;
        graph_check(&tail.item_type, &format!("tail {}", tail.name))?;
        insert_unique(&mut names, &tail.name)?;
        register_tail_scope(&mut scope, tail);
        if graph_reaches_variant(&tail.item_type) {
            return Err(ToolBuildError::VariantInInputPosition(tail.name.clone()));
        }
        if tail.verbatim && tail.separator.is_none() {
            return Err(ToolBuildError::VerbatimWithoutSeparator(tail.name.clone()));
        }
        if let Some(max) = tail.max
            && tail.min > max
        {
            return Err(ToolBuildError::InvalidTailOccurrenceBounds {
                name: tail.name.clone(),
                min: tail.min,
                max,
            });
        }
    }

    for constraint in &body.constraints {
        check_constraint::<VALUES>(constraint, &scope)?;
    }

    if let Some(result) = &body.result {
        graph_check(&result.type_, "result")?;
        for formatter in &result.formatters {
            check_identifier("formatter name", &formatter.name)?;
        }
        if !result
            .formatters
            .iter()
            .any(|f| f.name == result.default_formatter)
        {
            return Err(ToolBuildError::UnresolvedDefaultFormatter(
                result.default_formatter.clone(),
            ));
        }
    }

    for error_case in &body.errors {
        check_identifier("error-case name", &error_case.name)?;
        if let Some(payload) = &error_case.payload {
            graph_check(payload, &format!("error {}", error_case.name))?;
        }
    }

    Ok(())
}

pub(super) fn register_option_scope<S: ToolSchemaRepr>(
    scope: &mut NameScope<S>,
    opt: &ExtendedOptionSpec<S>,
) {
    scope.names.insert(opt.long.clone());
    scope.names.extend(opt.aliases.iter().cloned());
    let comparand = option_value_is_comparand(&opt.shape);
    scope.typed.insert(opt.long.clone(), comparand.clone());
    for alias in &opt.aliases {
        scope.typed.insert(alias.clone(), comparand.clone());
    }
}

fn check_subcommand_uniqueness(
    tool: &ExtendedToolType,
    node: &ExtendedCommandNode,
) -> Result<(), ToolBuildError> {
    let mut seen = BTreeSet::new();
    for sub in &node.subcommands {
        let child = &tool.commands[*sub as usize];
        insert_unique(&mut seen, &child.name)?;
        for alias in &child.aliases {
            insert_unique(&mut seen, alias)?;
        }
    }
    Ok(())
}

fn check_constraint<const VALUES: bool>(
    c: &ExtendedConstraint,
    scope: &NameScope,
) -> Result<(), ToolBuildError> {
    match c {
        ExtendedConstraint::RequiresAll(v)
        | ExtendedConstraint::AllOrNone(v)
        | ExtendedConstraint::RequiresAny(v) => check_refs::<VALUES>(v, scope),
        ExtendedConstraint::MutexGroups(groups) => {
            for g in groups {
                check_refs::<VALUES>(&g.refs, scope)?;
            }
            Ok(())
        }
        ExtendedConstraint::Implies(i) => {
            check_refs::<VALUES>(&i.lhs, scope)?;
            check_refs::<VALUES>(&i.rhs, scope)
        }
        ExtendedConstraint::Forbids(f) => {
            check_refs::<VALUES>(&f.lhs, scope)?;
            check_refs::<VALUES>(&f.rhs, scope)
        }
    }
}

fn check_refs<const VALUES: bool>(
    refs: &[ExtendedRef],
    scope: &NameScope,
) -> Result<(), ToolBuildError> {
    for r in refs {
        match r {
            ExtendedRef::Present(name) => {
                if !scope.names.contains(name) {
                    return Err(ToolBuildError::UnresolvedConstraintRef(name.clone()));
                }
            }
            ExtendedRef::ValueIs(v) => {
                if !VALUES {
                    return Err(ToolBuildError::UnresolvedValueIsLiteral(v.name.clone()));
                }
                if !scope.names.contains(&v.name) {
                    return Err(ToolBuildError::UnresolvedConstraintRef(v.name.clone()));
                }
                match scope.typed.get(&v.name) {
                    // A name with no value type (a flag) cannot carry a value-is.
                    None => return Err(ToolBuildError::ValueIsTypeMismatch(v.name.clone())),
                    // A repeatable-map whose type is not a map: the malformed
                    // type is reported by the structural checks; suppress the
                    // cascading value-is mismatch.
                    Some(ValueComparand::BlockedByTypeError) => {}
                    Some(ValueComparand::Type(comparand)) => match &v.value {
                        ExtendedValueIsLiteral::Resolved(value) => {
                            if !value_is_compatible(comparand, value) {
                                return Err(ToolBuildError::ValueIsTypeMismatch(v.name.clone()));
                            }
                        }
                        // The name is in scope, so composition (which carries the
                        // comparand type) should have resolved this literal. A
                        // surviving deferred literal is a resolution gap, not a
                        // silently acceptable ref.
                        ExtendedValueIsLiteral::Deferred(_) => {
                            return Err(ToolBuildError::UnresolvedValueIsLiteral(v.name.clone()));
                        }
                    },
                }
            }
        }
    }
    Ok(())
}

fn graph_reaches_variant(graph: &SchemaGraph) -> bool {
    let mut visited = BTreeSet::new();
    type_reaches_variant(graph, &graph.root, &mut visited)
}

/// Returns true if `ty` (resolving named references against `graph`) reaches a
/// [`SchemaType::Variant`]; `visited` guards recursive graphs.
fn type_reaches_variant(
    graph: &SchemaGraph,
    ty: &SchemaType,
    visited: &mut BTreeSet<String>,
) -> bool {
    if let SchemaType::Ref { id, .. } = ty
        && !visited.insert(id.to_string())
    {
        return false;
    }
    let Ok(resolved) = graph.resolve_ref(ty) else {
        return false;
    };
    match resolved {
        SchemaType::Variant { .. } => true,
        SchemaType::Record { fields, .. } => fields
            .iter()
            .any(|f| type_reaches_variant(graph, &f.body, visited)),
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            type_reaches_variant(graph, element, visited)
        }
        SchemaType::Option { inner, .. } => type_reaches_variant(graph, inner, visited),
        SchemaType::Map { key, value, .. } => {
            type_reaches_variant(graph, key, visited) || type_reaches_variant(graph, value, visited)
        }
        SchemaType::Tuple { elements, .. } => elements
            .iter()
            .any(|e| type_reaches_variant(graph, e, visited)),
        SchemaType::Result { spec, .. } => {
            spec.ok
                .as_deref()
                .is_some_and(|t| type_reaches_variant(graph, t, visited))
                || spec
                    .err
                    .as_deref()
                    .is_some_and(|t| type_reaches_variant(graph, t, visited))
        }
        SchemaType::Union { spec, .. } => spec
            .branches
            .iter()
            .any(|b| type_reaches_variant(graph, &b.body, visited)),
        SchemaType::Future { inner: Some(t), .. } | SchemaType::Stream { inner: Some(t), .. } => {
            type_reaches_variant(graph, t, visited)
        }
        _ => false,
    }
}
