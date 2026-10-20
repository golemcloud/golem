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

impl<S: ToolSchemaRepr> Default for ToolBuildCtx<S> {
    fn default() -> Self {
        Self {
            stack: Vec::new(),
            command_path: Vec::new(),
            inherited_globals: Vec::new(),
            graft_roots: Vec::new(),
        }
    }
}

impl<S: ToolSchemaRepr> ToolBuildCtx<S> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push_descriptor(&mut self, identity: impl Into<String>) -> Result<(), ToolBuildError> {
        let id = identity.into();
        if self.stack.contains(&id) {
            return Err(ToolBuildError::SubtreeCycle(self.cycle_path(&id)));
        }
        self.stack.push(id);
        Ok(())
    }
    pub fn pop_descriptor(&mut self) {
        self.stack.pop();
    }
    /// True when no ancestor descriptor is currently on the recursion stack —
    /// i.e. this is the outermost `__golem_tool_descriptor_for_*` invocation.
    /// Must be called from inside [`Self::with_descriptor`] (the current
    /// descriptor's identity is then on top of the stack), so a value of `1`
    /// means there is exactly one descriptor in flight: this one.
    ///
    /// A nested subtree child descriptor (called from a parent's subtree link)
    /// returns `false`. The child therefore skips composition/normalization and
    /// returns its raw command tree, with `value-is` literals still deferred;
    /// the outermost descriptor normalizes the fully grafted tree once, when all
    /// ancestor subtree globals and inherited-global de-projections are in scope.
    /// Normalizing a nested child against its standalone scope would resolve (and
    /// type-check) its constraints against child-local declarations that parent
    /// composition may widen or replace, rejecting a constraint that is valid in
    /// the composed tool.
    pub fn is_outermost_descriptor(&self) -> bool {
        self.stack.len() == 1
    }
    /// Build a child descriptor while the given identity is pushed on the
    /// recursion stack, always popping afterwards. Preferred over manual
    /// [`Self::push_descriptor`] / [`Self::pop_descriptor`] so an early return
    /// inside `f` cannot leak a stack entry and falsely report a later cycle.
    pub fn with_descriptor<T>(
        &mut self,
        identity: impl Into<String>,
        f: impl FnOnce(&mut Self) -> Result<T, ToolBuildError>,
    ) -> Result<T, ToolBuildError> {
        self.push_descriptor(identity)?;
        let result = f(self);
        self.pop_descriptor();
        result
    }
    pub fn inherited_globals(&self) -> &[EffectiveCommandField<S>] {
        &self.inherited_globals
    }
    pub fn with_inherited_globals<T>(
        &mut self,
        globals: Vec<EffectiveCommandField<S>>,
        f: impl FnOnce(&mut Self) -> Result<T, ToolBuildError>,
    ) -> Result<T, ToolBuildError> {
        let old_len = self.inherited_globals.len();
        self.inherited_globals.extend(globals);
        let result = f(self);
        self.inherited_globals.truncate(old_len);
        result
    }
    pub fn with_graft_root<T>(
        &mut self,
        expected_name: String,
        override_name: Option<String>,
        f: impl FnOnce(&mut Self) -> Result<T, ToolBuildError>,
    ) -> Result<T, ToolBuildError> {
        self.graft_roots.push(PendingGraftRoot {
            expected_name,
            override_name,
        });
        let result = f(self);
        self.graft_roots.pop();
        result
    }
    pub fn apply_pending_graft_root(
        &self,
        root: &mut ExtendedCommandNode<S>,
    ) -> Result<(), ToolBuildError> {
        let Some(pending) = self.graft_roots.last() else {
            return Ok(());
        };
        if pending.override_name.is_none() && root.name != pending.expected_name {
            return Err(ToolBuildError::SubtreeRootNameMismatch {
                expected: pending.expected_name.clone(),
                actual: root.name.clone(),
            });
        }
        if let Some(name) = &pending.override_name {
            root.name = name.clone();
        }
        Ok(())
    }
    fn cycle_path(&self, repeated: &str) -> String {
        let mut chain: Vec<&str> = self.stack.iter().map(String::as_str).collect();
        chain.push(repeated);
        chain.join(" -> ")
    }
    pub fn push_command(&mut self, name: impl Into<String>) {
        self.command_path.push(name.into());
    }
    pub fn pop_command(&mut self) {
        self.command_path.pop();
    }
    pub fn command_path(&self) -> &[String] {
        &self.command_path
    }
}

pub fn reconcile_subtree_parent_globals<S: ToolSchemaRepr>(
    mut parent_globals: ExtendedGlobals<S>,
    strict_ancestor_globals: &[EffectiveCommandField<S>],
    command_name: &str,
) -> Result<ExtendedGlobals<S>, ToolBuildError> {
    reconcile_globals(&mut parent_globals, strict_ancestor_globals, command_name)?;
    Ok(parent_globals)
}

pub fn reconcile_command_inherited_globals<S: ToolSchemaRepr>(
    node: &mut ExtendedCommandNode<S>,
    strict_ancestor_globals: &[EffectiveCommandField<S>],
    command_name: &str,
) -> Result<(), ToolBuildError> {
    reconcile_globals(&mut node.globals, strict_ancestor_globals, command_name)?;
    if let Some(body) = node.body.as_mut() {
        reconcile_body(body, strict_ancestor_globals, command_name)?;
    }
    Ok(())
}

/// Turn a child subtree's command list into the graft-local nodes to splice
/// beneath a parent. The child root (index 0) becomes the parent's subtree
/// command: its `globals` and `subcommands` are preserved (so recursive
/// globals still apply and the graft-local child indices stay valid), and its
/// `name`/`doc`/`aliases` may be overridden by the parent's `#[command(...)]`.
///
/// The child root may carry a body — the child trait's implicit-body method
/// (e.g. `git stash` runs the `stash` body while `git stash pop` walks to the
/// `pop` child). The subtree method's propagating params (`parent_globals`)
/// are first reconciled against `strict_ancestor_globals`, then the child root's
/// own globals and body are reconciled against the full inherited set. A
/// compatible same-name re-declaration is de-projected onto the inherited
/// global; an incompatible one is rejected as `InheritedGlobalConflict`.
/// Surviving `parent_globals` are then prepended onto the grafted root's
/// globals so they propagate to every descendant subcommand.
///
/// `expected_name` is the parent subtree method's command name; unless an
/// explicit `override_name` is given, the child root name must equal it
/// (`SubtreeRootNameMismatch`). Command-annotations are not supported on a
/// subtree command (the model places them on a command body), so a non-`None`
/// `override_annotations` is rejected rather than silently dropped.
///
/// Command indices are returned unchanged (graft-local); the final offset into
/// the parent's command tree is applied by [`append_grafted_subtree`].
pub fn graft_subtree<S: ToolSchemaRepr>(
    child: ExtendedToolType<S>,
    expected_name: &str,
    parent_globals: ExtendedGlobals<S>,
    strict_ancestor_globals: &[EffectiveCommandField<S>],
    override_name: Option<String>,
    override_doc: Option<Doc>,
    override_aliases: Option<Vec<String>>,
    override_annotations: Option<CommandAnnotations>,
) -> Result<Vec<ExtendedCommandNode<S>>, ToolBuildError> {
    let mut nodes = child.commands;
    let root = nodes.first_mut().ok_or(ToolBuildError::EmptyCommandTree)?;
    if override_annotations.is_some() {
        return Err(ToolBuildError::SubtreeAnnotationsUnsupported(
            override_name.clone().unwrap_or_else(|| root.name.clone()),
        ));
    }
    if override_name.is_none() && root.name != expected_name {
        return Err(ToolBuildError::SubtreeRootNameMismatch {
            expected: expected_name.to_string(),
            actual: root.name.clone(),
        });
    }
    // Apply overrides first so any reconciliation error reports the final
    // grafted command name rather than the standalone child root name.
    if let Some(name) = override_name {
        root.name = name;
    }
    if let Some(doc) = override_doc {
        root.doc = doc;
    }
    if let Some(aliases) = override_aliases {
        root.aliases = aliases;
    }

    let command_name = root.name.clone();

    // The subtree method's params become propagating globals on the grafted
    // root. They are inherited from the parent command, so first reconcile them
    // against strict ancestors above the graft point, then reconcile the grafted
    // root's own globals/body against the full inherited set. Doing both here
    // preserves the normal inherited-global contract even though the parent
    // globals will be stored as same-node globals on the grafted root.
    let parent_globals =
        reconcile_subtree_parent_globals(parent_globals, strict_ancestor_globals, &command_name)?;

    let mut inherited = strict_ancestor_globals.to_vec();
    inherited.extend(
        parent_globals
            .options
            .iter()
            .cloned()
            .map(EffectiveCommandField::Option),
    );
    inherited.extend(
        parent_globals
            .flags
            .iter()
            .cloned()
            .map(EffectiveCommandField::Flag),
    );

    reconcile_globals(&mut root.globals, &inherited, &command_name)?;
    if let Some(body) = root.body.as_mut() {
        reconcile_body(body, &inherited, &command_name)?;
    }

    // Prepend the parent globals so they propagate to every descendant
    // subcommand of the grafted root. Globals and subcommands keep their
    // graft-local indices; `append_grafted_subtree` shifts them on append.
    let mut opts = parent_globals.options;
    opts.append(&mut root.globals.options);
    root.globals.options = opts;

    let mut flags = parent_globals.flags;
    flags.append(&mut root.globals.flags);
    root.globals.flags = flags;

    Ok(nodes)
}

/// Normalize a whole tool's command tree against inherited globals.
///
/// A subtree child trait is synthesized independently, so a child command whose
/// Rust signature repeats a parameter an ancestor declares as a global projects
/// it as a body option/flag/positional (or as its own global) in the standalone
/// descriptor. Likewise a leaf command in the same trait as the root may repeat
/// a root global. Once composed under the ancestor that supplies that name as a
/// global, the local re-declaration must be reconciled, otherwise the canonical
/// shape carries a body-local (or nested-global) name colliding with an
/// effective inherited global.
///
/// This is the single source of truth for that reconciliation. It traverses the
/// tree root→leaf, carrying the *strict ancestor* globals in scope. For every
/// node it reconciles the node's own globals and its body arguments against the
/// strict-ancestor globals:
///
/// * a same-name re-declaration whose canonical input shape is *compatible* with
///   the inherited global is removed — the ancestor global is the single source
///   of truth for docs, defaults, requiredness, aliases, and parse behavior;
/// * a same-name re-declaration whose shape is *incompatible* is an
///   [`ToolBuildError::InheritedGlobalConflict`]: the composition is invalid and
///   is rejected rather than silently dropping or replacing the local parameter.
///
/// Body arguments are reconciled only against *strict ancestors*, never the
/// node's own globals; a body argument colliding with a global declared on the
/// same command is an ordinary authoring error left for [`validate_tool`].
///
/// The traversal guards against malformed (cyclic / out-of-bounds) trees so it
/// is safe to run before [`validate_tool`] proves the tree well-formed.
pub fn normalize_inherited_globals<S: ToolSchemaRepr>(
    tool: &mut ExtendedToolType<S>,
) -> Result<(), ToolBuildError> {
    if tool.commands.is_empty() {
        return Ok(());
    }
    let mut visited = vec![false; tool.commands.len()];
    normalize_command(tool, 0, &[], &mut visited)
}

fn normalize_command<S: ToolSchemaRepr>(
    tool: &mut ExtendedToolType<S>,
    index: usize,
    ancestor_globals: &[EffectiveCommandField<S>],
    visited: &mut [bool],
) -> Result<(), ToolBuildError> {
    if index >= tool.commands.len() || visited[index] {
        return Ok(());
    }
    visited[index] = true;

    let command_name = tool.commands[index].name.clone();

    // Reconcile this node's own globals and body args against strict ancestors.
    {
        let node = &mut tool.commands[index];
        reconcile_globals(&mut node.globals, ancestor_globals, &command_name)?;
        if let Some(body) = node.body.as_mut() {
            reconcile_body(body, ancestor_globals, &command_name)?;
        }
    }

    // Resolve any deferred `value-is` literals now that the constraint scope —
    // strict-ancestor globals plus this node's own (surviving) globals and body
    // arguments — is known. A subtree child trait names a constraint against a
    // global supplied by an ancestor subtree method; the standalone child could
    // not type the literal, so it was deferred until this composition step.
    if tool.commands[index].body.is_some() {
        let node = &mut tool.commands[index];
        let scope = value_is_scope(ancestor_globals, &node.globals, node.body.as_ref().unwrap());
        resolve_deferred_value_is(node.body.as_mut().unwrap(), &scope)?;
    }

    // Children inherit the strict-ancestor globals plus this node's surviving
    // globals (the ones not removed as compatible re-declarations).
    let mut child_globals = ancestor_globals.to_vec();
    {
        let node = &tool.commands[index];
        child_globals.extend(
            node.globals
                .options
                .iter()
                .cloned()
                .map(EffectiveCommandField::Option),
        );
        child_globals.extend(
            node.globals
                .flags
                .iter()
                .cloned()
                .map(EffectiveCommandField::Flag),
        );
    }

    let subcommands = tool.commands[index].subcommands.clone();
    for sub in subcommands {
        if sub >= 0 {
            normalize_command(tool, sub as usize, &child_globals, visited)?;
        }
    }
    Ok(())
}

/// Builds the `value-is` resolution scope for a command body: strict-ancestor
/// globals, the node's own globals, and the body's own arguments. This mirrors
/// the constraint scope assembled by [`check_body`] (via [`register_option_scope`]
/// and the positional/tail/flag handling) so deferred-literal resolution and
/// validation agree on which names are value-carrying and on each name's
/// comparand graph.
fn value_is_scope<S: ToolSchemaRepr>(
    ancestors: &[EffectiveCommandField<S>],
    node_globals: &ExtendedGlobals<S>,
    body: &ExtendedCommandBody<S>,
) -> NameScope<S> {
    let mut scope = NameScope::default();
    for field in ancestors {
        match field {
            EffectiveCommandField::Option(opt) => register_option_scope(&mut scope, opt),
            EffectiveCommandField::Flag(flag) => register_flag_scope(&mut scope, flag),
        }
    }
    for opt in &node_globals.options {
        register_option_scope(&mut scope, opt);
    }
    for flag in &node_globals.flags {
        register_flag_scope(&mut scope, flag);
    }
    for opt in &body.options {
        register_option_scope(&mut scope, opt);
    }
    for flag in &body.flags {
        register_flag_scope(&mut scope, flag);
    }
    for positional in &body.positionals.fixed {
        register_fixed_positional_scope(&mut scope, positional);
    }
    if let Some(tail) = &body.positionals.tail {
        register_tail_scope(&mut scope, tail);
    }
    scope
}

/// Resolves every [`ExtendedValueIsLiteral::Deferred`] literal in `body`'s
/// constraints against `scope`, the effective constraint scope assembled by
/// [`value_is_scope`] (and mirrored by [`check_body`]). This is the single
/// source of truth for typing a `value-is` literal: the descriptor macro carries
/// the raw literal and never re-derives a comparand graph, so resolution always
/// agrees with the validation performed by [`check_refs`].
///
/// For a name with a value-carrying comparand graph the literal is interpreted
/// into a [`SchemaValue`] and then checked against the graph with
/// [`value_is_compatible`], so a literal whose *value* is incompatible (a wrong
/// type or one that violates the option's refinements — e.g. a regex/numeric
/// bound) is rejected here rather than slipping through to a later stage. A name
/// in scope but without a comparand (a flag) is a
/// [`ToolBuildError::ValueIsTypeMismatch`]. A name not in scope is left deferred
/// — it is reported as an unresolved constraint reference by [`check_refs`] (the
/// standalone subtree-child case where the ancestor global is not present).
fn resolve_deferred_value_is<S: ToolSchemaRepr>(
    body: &mut ExtendedCommandBody<S>,
    scope: &NameScope<S>,
) -> Result<(), ToolBuildError> {
    for constraint in &mut body.constraints {
        for_each_ref_mut(constraint, &mut |r| {
            let ExtendedRef::ValueIs(v) = r else {
                return Ok(());
            };
            let ExtendedValueIsLiteral::Deferred(lit) = &v.value else {
                return Ok(());
            };
            match scope.typed.get(&v.name) {
                Some(ValueComparand::Type(comparand)) => {
                    // The structural validator owns schema soundness. If the
                    // comparand graph is unsound (a dangling or pure-alias-cycle
                    // ref) leave the literal deferred so `validate_tool` reports
                    // the real schema error instead of a cascading value-is
                    // mismatch against a graph that cannot be resolved.
                    if comparand.graph.is_sound() {
                        let value = comparand
                            .graph
                            .value_is_literal(lit, comparand.mode == ValueIsMode::WholeOrOnePeel)
                            .map_err(|_| ToolBuildError::ValueIsTypeMismatch(v.name.clone()))?;
                        v.value = ExtendedValueIsLiteral::Resolved(value);
                    }
                }
                // A repeatable-map whose type is not a map: `validate_tool`
                // reports the malformed type. Leave the literal deferred and
                // suppress the cascading value-is mismatch.
                Some(ValueComparand::BlockedByTypeError) => {}
                // A flag (in scope, no value type) cannot carry a value-is. A
                // name not in scope is left deferred — `check_refs` reports it as
                // an unresolved constraint reference.
                None => {
                    if scope.names.contains(&v.name) {
                        return Err(ToolBuildError::ValueIsTypeMismatch(v.name.clone()));
                    }
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// Applies `f` to every [`ExtendedRef`] referenced by a constraint, regardless of
/// its variant, short-circuiting on the first error.
fn for_each_ref_mut<S: ToolSchemaRepr>(
    constraint: &mut ExtendedConstraint<S>,
    f: &mut impl FnMut(&mut ExtendedRef<S>) -> Result<(), ToolBuildError>,
) -> Result<(), ToolBuildError> {
    match constraint {
        ExtendedConstraint::RequiresAll(v)
        | ExtendedConstraint::AllOrNone(v)
        | ExtendedConstraint::RequiresAny(v) => {
            for r in v.iter_mut() {
                f(r)?;
            }
        }
        ExtendedConstraint::MutexGroups(groups) => {
            for group in groups.iter_mut() {
                for r in group.refs.iter_mut() {
                    f(r)?;
                }
            }
        }
        ExtendedConstraint::Implies(i) => {
            for r in i.lhs.iter_mut() {
                f(r)?;
            }
            for r in i.rhs.iter_mut() {
                f(r)?;
            }
        }
        ExtendedConstraint::Forbids(fb) => {
            for r in fb.lhs.iter_mut() {
                f(r)?;
            }
            for r in fb.rhs.iter_mut() {
                f(r)?;
            }
        }
    }
    Ok(())
}

fn reconcile_globals<S: ToolSchemaRepr>(
    globals: &mut ExtendedGlobals<S>,
    ancestors: &[EffectiveCommandField<S>],
    command: &str,
) -> Result<(), ToolBuildError> {
    if ancestors.is_empty() {
        return Ok(());
    }
    let mut kept_options = Vec::with_capacity(globals.options.len());
    for opt in std::mem::take(&mut globals.options) {
        let shape = FieldShape::Value(option_collected_graph(&opt.shape));
        if !reconcile_local(&option_surface_names(&opt), &shape, ancestors, command)? {
            kept_options.push(opt);
        }
    }
    globals.options = kept_options;

    let mut kept_flags = Vec::with_capacity(globals.flags.len());
    for flag in std::mem::take(&mut globals.flags) {
        let shape = flag_field_shape(&flag);
        if !reconcile_local(&flag_surface_names(&flag), &shape, ancestors, command)? {
            kept_flags.push(flag);
        }
    }
    globals.flags = kept_flags;
    Ok(())
}

fn reconcile_body<S: ToolSchemaRepr>(
    body: &mut ExtendedCommandBody<S>,
    ancestors: &[EffectiveCommandField<S>],
    command: &str,
) -> Result<(), ToolBuildError> {
    if ancestors.is_empty() {
        return Ok(());
    }
    let mut kept_options = Vec::with_capacity(body.options.len());
    for opt in std::mem::take(&mut body.options) {
        let shape = FieldShape::Value(option_collected_graph(&opt.shape));
        if !reconcile_local(&option_surface_names(&opt), &shape, ancestors, command)? {
            kept_options.push(opt);
        }
    }
    body.options = kept_options;

    let mut kept_flags = Vec::with_capacity(body.flags.len());
    for flag in std::mem::take(&mut body.flags) {
        let shape = flag_field_shape(&flag);
        if !reconcile_local(&flag_surface_names(&flag), &shape, ancestors, command)? {
            kept_flags.push(flag);
        }
    }
    body.flags = kept_flags;

    let mut kept_fixed = Vec::with_capacity(body.positionals.fixed.len());
    for positional in std::mem::take(&mut body.positionals.fixed) {
        let shape = FieldShape::Value(positional.type_.clone());
        if !reconcile_local(
            std::slice::from_ref(&positional.name),
            &shape,
            ancestors,
            command,
        )? {
            kept_fixed.push(positional);
        }
    }
    body.positionals.fixed = kept_fixed;

    if let Some(tail) = body.positionals.tail.take() {
        let shape = FieldShape::Value(tail.item_type.list());
        if !reconcile_local(std::slice::from_ref(&tail.name), &shape, ancestors, command)? {
            body.positionals.tail = Some(tail);
        }
    }

    // De-projection may have removed the parameter that was the tail (or a later
    // positional that kept an earlier `Vec<T>` out of tail position), so re-infer
    // the tail against the parameters that actually survived.
    reinfer_body_tail(body)?;
    Ok(())
}

/// Finalize a command body's tail positional after [`reconcile_body`] removed the
/// inherited re-declarations that did not survive in scope.
///
/// The macro emits only each `Vec<T>` candidate's *selected* surface into the body
/// (tail positional or repeatable-list option) and records the candidate, in
/// declaration order, in [`ExtendedCommandBody::positional_plan`] (§5.8: the
/// *last* positional `Vec<T>` is the tail, an earlier one is a repeatable-list
/// option). Because the selection assumed the macro-known inherited
/// re-declarations would de-project, this pass repairs it against the parameters
/// that actually survived. It:
///
/// * **demotes** an installed tail into a repeatable-list option (reconstructed by
///   copying the tail's value graph) when another positional survived after it —
///   rejecting an explicitly authored tail, or an inferred tail carrying
///   occurrence bounds / tail-only attributes a repeatable-list option cannot
///   represent; and
/// * **promotes** the last surviving `Vec<T>` candidate's repeatable-list option
///   into the tail (reconstructed likewise) when its natural tail was
///   de-projected — rejecting one carrying option-only attributes a tail cannot
///   represent.
///
/// It is a no-op for hand-built bodies (empty plan) and whenever the natural tail
/// is already the last surviving positional.
pub(super) fn reinfer_body_tail<S: ToolSchemaRepr>(
    body: &mut ExtendedCommandBody<S>,
) -> Result<(), ToolBuildError> {
    if body.positional_plan.is_empty() {
        return Ok(());
    }
    // The last surviving positional-eligible candidate, in declaration order.
    let mut last = None;
    for (idx, candidate) in body.positional_plan.iter().enumerate() {
        if body_contains_positional(body, candidate.name()) {
            last = Some(idx);
        }
    }
    let Some(last_idx) = last else { return Ok(()) };
    let last_name = body.positional_plan[last_idx].name().to_string();

    // An explicitly-authored tail that survives de-projection must be the last
    // positional-eligible candidate. If a positional survives after it, its
    // authored order is violated — whether it still holds the tail slot or was
    // lowered to an inherited-global surrogate option (a form invisible to the
    // demote path below, which only sees an installed tail). An explicit tail that
    // was de-projected entirely is gone and no longer constrains: a genuine later
    // `Vec<T>` tail may legitimately take the slot.
    if body.positional_plan[..last_idx].iter().any(|c| {
        matches!(
            c,
            PositionalCandidate::VecCandidate {
                explicit_tail: true,
                ..
            }
        ) && body_contains_positional(body, c.name())
    }) {
        return Err(ToolBuildError::FixedPositionalAfterTail(last_name));
    }

    // Demote first: an installed inferred tail whose declaration precedes a
    // surviving positional is no longer last (§5.8). Doing this before promotion
    // means that if the last candidate is itself promoted (and its option removed)
    // the demoted option is still inserted relative to the remaining later options.
    if let Some(tail_name) = body.positionals.tail.as_ref().map(|t| t.name.clone())
        && let Some(tail_idx) = body
            .positional_plan
            .iter()
            .position(|c| c.name() == tail_name)
        && tail_idx < last_idx
    {
        demote_tail_to_option(body, tail_idx)?;
    }

    // Promote: the last surviving candidate must hold the tail slot. When it is a
    // `Vec<T>` currently projected as a repeatable-list option (its natural tail
    // was de-projected), move it into the tail slot.
    if matches!(
        body.positional_plan[last_idx],
        PositionalCandidate::VecCandidate { .. }
    ) {
        promote_option_to_tail(body, last_idx)?;
    }
    Ok(())
}

/// Demote the body's installed inferred tail (the candidate at `tail_idx`) into a
/// repeatable-list option, because a positional survives after it. The option is
/// reconstructed from the tail's value graph and inserted in declaration order
/// among the body options. Rejects an inferred tail whose authored occurrence
/// bounds (`min`/`max`) or tail-only attributes a repeatable-list option cannot
/// represent. (An explicit tail before a survivor is rejected earlier, in
/// [`reinfer_body_tail`].)
fn demote_tail_to_option<S: ToolSchemaRepr>(
    body: &mut ExtendedCommandBody<S>,
    tail_idx: usize,
) -> Result<(), ToolBuildError> {
    let PositionalCandidate::VecCandidate {
        name,
        has_min_or_max_attr,
        later_option_names,
        ..
    } = &body.positional_plan[tail_idx]
    else {
        return Ok(());
    };
    let name = name.clone();
    let has_min_or_max_attr = *has_min_or_max_attr;
    let later_option_names = later_option_names.clone();

    // Only act on the candidate that actually holds the tail slot.
    if body.positionals.tail.as_ref().map(|t| &t.name) != Some(&name) {
        return Ok(());
    }

    // `min`/`max` are overloaded between surfaces — occurrence bounds on a tail,
    // item numeric bounds on a repeatable-list option — so a candidate that
    // authored either cannot switch surface without changing their meaning. This
    // consults the authored fact rather than the materialized tail shape because
    // an authored `min = 0` coincides with the tail default and so leaves no trace
    // in the tail's `min`/`max` fields.
    if has_min_or_max_attr {
        return Err(ToolBuildError::VecSurfaceConflict {
            name,
            reason: "it authored a `min`/`max` bound, which means occurrence count \
                     on a tail positional but item count on a repeatable-list \
                     option; a parameter now follows it so it must become a \
                     repeatable-list option, which would change that meaning"
                .to_string(),
        });
    }

    let tail = body
        .positionals
        .tail
        .as_ref()
        .expect("tail name matched above");
    // A repeatable-list option has no separator/verbatim/stdio handling, so a
    // tail using any of those cannot be represented as one.
    if tail.separator.is_some() || tail.verbatim || tail.accepts_stdio {
        return Err(ToolBuildError::VecSurfaceConflict {
            name,
            reason: "it has a tail-only attribute (`separator`/`verbatim`/`accepts_stdio`) \
                     that a repeatable-list option cannot express, but a parameter now \
                     follows it so it must become a repeatable-list option"
                .to_string(),
        });
    }

    let tail = body
        .positionals
        .tail
        .take()
        .expect("tail name matched above");
    let option = ExtendedOptionSpec {
        long: tail.name,
        short: None,
        aliases: Vec::new(),
        doc: tail.doc,
        value_name: tail.value_name,
        shape: ExtendedOptionShape::RepeatableList(ExtendedRepeatableListShape {
            repetition: wire::Repetition::Repeated,
            item_type: tail.item_type,
        }),
        default: None,
        required: false,
        env_var: None,
    };
    // §D7: body options are in declaration order. Insert before the first option
    // declared after this `Vec<T>` that survived de-projection; otherwise append.
    let insert_at = later_option_names
        .iter()
        .find_map(|later| body.options.iter().position(|o| &o.long == later));
    match insert_at {
        Some(pos) => body.options.insert(pos, option),
        None => body.options.push(option),
    }
    Ok(())
}

/// Promote the last surviving `Vec<T>` candidate (at `last_idx`) from a
/// repeatable-list option into the tail positional, because its natural tail was
/// de-projected. An explicit tail lowered to an inherited-global surrogate option
/// installs its full authored spec (`authored_tail_surrogate`) verbatim, so its
/// tail-only attributes are preserved; an inferred candidate is reconstructed
/// from the option's value graph instead. A no-op if it already holds the tail
/// slot or never projected to an option; rejects an inferred candidate carrying
/// option-only attributes (or an `Option<Vec<T>>`, or authored `min`/`max`) that
/// a tail cannot represent.
fn promote_option_to_tail<S: ToolSchemaRepr>(
    body: &mut ExtendedCommandBody<S>,
    last_idx: usize,
) -> Result<(), ToolBuildError> {
    let PositionalCandidate::VecCandidate {
        name,
        optional_vec,
        has_min_or_max_attr,
        authored_tail_surrogate,
        ..
    } = &body.positional_plan[last_idx]
    else {
        return Ok(());
    };
    let name = name.clone();
    let optional_vec = *optional_vec;
    let has_min_or_max_attr = *has_min_or_max_attr;
    let authored_tail_surrogate = authored_tail_surrogate.clone();

    // Already the tail (its natural tail survived): nothing to do.
    if body
        .positionals
        .tail
        .as_ref()
        .is_some_and(|t| t.name == name)
    {
        return Ok(());
    }
    let Some(pos) = body.options.iter().position(|o| o.long == name) else {
        return Ok(());
    };

    // An explicit tail lowered to an inherited-global surrogate option: install
    // its full authored spec verbatim. The surrogate option carries none of the
    // tail-only fields (`separator`/`verbatim`/`accepts_stdio`/occurrence bounds),
    // so reconstructing from it would silently drop them; the authored spec keeps
    // them (and any resulting invalid state, e.g. `verbatim` without `separator`,
    // is caught later by `validate_tool`). `min`/`max` are already occurrence
    // bounds here, so the inferred-only `has_min_or_max_attr` rejection is skipped.
    if let Some(authored_tail) = authored_tail_surrogate {
        // Defensive against hand-built bodies: confirm the option really is the
        // droppable repeatable-list surrogate before replacing it with the tail.
        verify_promotable_surrogate(&body.options[pos], &name)?;
        body.options.remove(pos);
        body.positionals.tail = Some(*authored_tail);
        return Ok(());
    }

    // A tail is variadic with no absent state and interprets `min`/`max` as
    // occurrence bounds, so these authored shapes cannot move onto a tail.
    if optional_vec {
        return Err(ToolBuildError::VecSurfaceConflict {
            name,
            reason: "it is an `Option<Vec<T>>`, which has no tail-positional \
                     representation, but de-projection made it the last positional \
                     so it must become the tail"
                .to_string(),
        });
    }
    if has_min_or_max_attr {
        return Err(ToolBuildError::VecSurfaceConflict {
            name,
            reason: "it has a `min`/`max` bound applied to its items as a \
                     repeatable-list option, but de-projection made it the last \
                     positional so it must become the tail (where `min`/`max` would \
                     instead bound the occurrence count)"
                .to_string(),
        });
    }
    verify_promotable_surrogate(&body.options[pos], &name)?;

    let option = body.options.remove(pos);
    let item_type = match option.shape {
        ExtendedOptionShape::RepeatableList(list) => list.item_type,
        _ => unreachable!("shape checked as RepeatableList above"),
    };
    body.positionals.tail = Some(ExtendedTailPositional {
        name: option.long,
        doc: option.doc,
        value_name: option.value_name,
        item_type,
        min: 0,
        max: None,
        separator: None,
        verbatim: false,
        accepts_stdio: false,
    });
    Ok(())
}

/// Verify that `option` is a droppable repeatable-list surrogate that can take the
/// tail slot: it must carry no option-only surface a tail positional cannot
/// express (`short`/`aliases`/`default`/`required`/`env`), and must be a
/// `RepeatableList` with `Repeated` (non-delimited) repetition.
fn verify_promotable_surrogate<S: ToolSchemaRepr>(
    option: &ExtendedOptionSpec<S>,
    name: &str,
) -> Result<(), ToolBuildError> {
    if option.short.is_some()
        || !option.aliases.is_empty()
        || option.default.is_some()
        || option.required
        || option.env_var.is_some()
    {
        return Err(ToolBuildError::VecSurfaceConflict {
            name: name.to_string(),
            reason: "it has an option-only attribute (`short`/`aliases`/`default`/\
                     `required`/`env`) that a tail positional cannot express, but \
                     de-projection made it the last positional so it must become the tail"
                .to_string(),
        });
    }
    let ExtendedOptionShape::RepeatableList(list) = &option.shape else {
        return Err(ToolBuildError::VecSurfaceConflict {
            name: name.to_string(),
            reason: "it does not project to a repeatable list, so it has no \
                     tail-positional representation"
                .to_string(),
        });
    };
    if !matches!(list.repetition, wire::Repetition::Repeated) {
        return Err(ToolBuildError::VecSurfaceConflict {
            name: name.to_string(),
            reason: "it uses a delimited repetition that a tail positional cannot \
                     express, but de-projection made it the last positional so it \
                     must become the tail"
                .to_string(),
        });
    }
    Ok(())
}

/// Whether the body still carries a positional-eligible parameter with the given
/// surface name (as a fixed positional, the tail, or a repeatable-list option),
/// i.e. it survived de-projection.
fn body_contains_positional<S: ToolSchemaRepr>(body: &ExtendedCommandBody<S>, name: &str) -> bool {
    body.positionals.fixed.iter().any(|p| p.name == name)
        || body
            .positionals
            .tail
            .as_ref()
            .is_some_and(|t| t.name == name)
        || body.options.iter().any(|o| o.long == name)
}

/// Decide the fate of one local declaration against the inherited globals in
/// scope. Returns `Ok(true)` when the local is a compatible re-declaration that
/// should be removed (the inherited global covers it), `Ok(false)` when no
/// inherited global shares a surface name (keep the local), and
/// [`ToolBuildError::InheritedGlobalConflict`] when a surface name matches but
/// the input shapes are incompatible.
fn reconcile_local<S: ToolSchemaRepr>(
    local_names: &[String],
    local_shape: &FieldShape<S>,
    ancestors: &[EffectiveCommandField<S>],
    command: &str,
) -> Result<bool, ToolBuildError> {
    // A local may share a surface name with more than one inherited global (its
    // long name with one, an alias with another). All ancestors are scanned so
    // that an incompatible collision — even one found after a compatible one — is
    // reported immediately as a conflict. The local can be de-projected onto an
    // inherited global only when it collides with exactly one *distinct* inherited
    // global (matching the same global through both its long name and an alias is
    // a single ancestor entry, hence still one global). Colliding compatibly with
    // two or more distinct inherited globals is ambiguous — there is no single
    // global to inherit from, and silently dropping the local would leave the Rust
    // parameter with no canonical body field — so that is also a conflict.
    let mut compatible: Vec<String> = Vec::new();
    for inherited in ancestors {
        let inherited_names = effective_field_surface_names(inherited);
        let Some(colliding) = local_names
            .iter()
            .find(|l| inherited_names.iter().any(|n| n == *l))
        else {
            continue;
        };
        if !field_shapes_compatible(&effective_field_shape(inherited), local_shape) {
            return Err(ToolBuildError::InheritedGlobalConflict {
                name: colliding.clone(),
                inherited: effective_field_primary_name(inherited),
                command: command.to_string(),
            });
        }
        let primary = effective_field_primary_name(inherited);
        if !compatible.iter().any(|p| p == &primary) {
            compatible.push(primary);
        }
    }
    if compatible.len() > 1 {
        return Err(ToolBuildError::InheritedGlobalConflict {
            name: local_names.first().cloned().unwrap_or_default(),
            inherited: compatible.join(", "),
            command: command.to_string(),
        });
    }
    Ok(!compatible.is_empty())
}

/// The primary (long) surface name of an inherited effective global, used to
/// name the colliding global in [`ToolBuildError::InheritedGlobalConflict`].
fn effective_field_primary_name<S: ToolSchemaRepr>(g: &EffectiveCommandField<S>) -> String {
    match g {
        EffectiveCommandField::Option(o) => o.long.clone(),
        EffectiveCommandField::Flag(f) => f.long.clone(),
    }
}

/// The canonical input "surface family" of a command field, used to decide
/// whether a local re-declaration is compatible with an inherited global. Flags
/// are distinguished by their flag family (a bool flag and a count flag carry
/// different values, and neither is interchangeable with a value-bearing option
/// or positional of the same name); every value-bearing form (scalar/optional
/// option, repeatable list/map option, fixed positional, tail positional) is
/// compared by its canonical input value graph.
#[allow(clippy::large_enum_variant)]
enum FieldShape<S: ToolSchemaRepr = SchemaGraph> {
    BoolFlag,
    CountFlag,
    Value(S),
}

fn flag_field_shape<S: ToolSchemaRepr>(f: &FlagSpec) -> FieldShape<S> {
    match f.shape {
        wire::FlagShape::BoolFlag(_) => FieldShape::BoolFlag,
        wire::FlagShape::CountFlag(_) => FieldShape::CountFlag,
    }
}

fn effective_field_shape<S: ToolSchemaRepr>(g: &EffectiveCommandField<S>) -> FieldShape<S> {
    match g {
        EffectiveCommandField::Option(o) => FieldShape::Value(option_collected_graph(&o.shape)),
        EffectiveCommandField::Flag(f) => flag_field_shape(f),
    }
}

fn field_shapes_compatible<S: ToolSchemaRepr>(a: &FieldShape<S>, b: &FieldShape<S>) -> bool {
    match (a, b) {
        (FieldShape::BoolFlag, FieldShape::BoolFlag) => true,
        (FieldShape::CountFlag, FieldShape::CountFlag) => true,
        (FieldShape::Value(ga), FieldShape::Value(gb)) => ga.shape_matches(gb),
        _ => false,
    }
}

fn option_surface_names<S: ToolSchemaRepr>(o: &ExtendedOptionSpec<S>) -> Vec<String> {
    let mut names = Vec::with_capacity(1 + o.aliases.len());
    names.push(o.long.clone());
    names.extend(o.aliases.iter().cloned());
    names
}

fn flag_surface_names(f: &FlagSpec) -> Vec<String> {
    let mut names = Vec::with_capacity(1 + f.aliases.len());
    names.push(f.long.clone());
    names.extend(f.aliases.iter().cloned());
    names
}

fn effective_field_surface_names<S: ToolSchemaRepr>(g: &EffectiveCommandField<S>) -> Vec<String> {
    match g {
        EffectiveCommandField::Option(o) => option_surface_names(o),
        EffectiveCommandField::Flag(f) => flag_surface_names(f),
    }
}

/// Append a graft (graft-local command nodes whose index 0 is the dispatcher
/// placeholder) to `parent`, offsetting every internal subcommand index, and
/// return the parent index of the placeholder. The caller links this index as a
/// subcommand of the hosting command.
pub fn append_grafted_subtree<S: ToolSchemaRepr>(
    parent: &mut Vec<ExtendedCommandNode<S>>,
    mut graft: Vec<ExtendedCommandNode<S>>,
) -> i32 {
    let offset = parent.len() as i32;
    for node in &mut graft {
        node.subcommands = node.subcommands.iter().map(|i| i + offset).collect();
    }
    parent.extend(graft);
    offset
}
