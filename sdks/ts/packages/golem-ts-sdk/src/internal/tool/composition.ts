// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { CodecShapeMismatchError, type FluentCodec } from '../../fluent/schema/codec';
import { cloneSchemaValue, schemaShapesMatch } from '../schema-model';
import { toolBuildError } from './errors';
import {
  type Doc,
  type EffectiveCommandField,
  type ExtendedCommandBody,
  type ExtendedCommandNode,
  type ExtendedConstraint,
  type ExtendedGlobals,
  type ExtendedOptionSpec,
  type ExtendedRef,
  type ExtendedTailPositional,
  type ValueIsMode,
  ExtendedToolType,
  emptyGlobals,
  listCodec,
  optionCollectedCodec,
  optionValueCodec,
  valueIsCodecs,
} from './model';
import { parseSourceValue, schemaValueConforms, validateExtendedTool } from './validation';

interface ValueIsScopeEntry {
  readonly codec?: FluentCodec;
  readonly mode?: ValueIsMode;
}

export interface GraftSubtreeOptions {
  readonly expectedName: string;
  readonly parentGlobals?: ExtendedGlobals;
  readonly name?: string;
  readonly aliases?: readonly string[];
  readonly doc?: Doc;
}

/**
 * Clone a child tree for attachment below a parent command. The child root can
 * be renamed/documented and receives the subtree command's globals. The clone
 * is marked for inherited-global reconciliation when the composed tree is
 * normalized; the independently reusable child remains unchanged.
 */
export function graftSubtree(
  child: ExtendedToolType,
  options: GraftSubtreeOptions,
): ExtendedCommandNode {
  if (options.name === undefined && child.root.name !== options.expectedName) {
    toolBuildError(
      'subtree-root-name-mismatch',
      `subtree root name "${child.root.name}" does not match the parent command name "${options.expectedName}"`,
    );
  }
  let root = cloneCommandTree(child.root);
  const parentGlobals = options.parentGlobals ?? emptyGlobals();
  root = {
    ...root,
    name: options.name ?? root.name,
    aliases: options.aliases ? [...options.aliases] : root.aliases,
    doc: options.doc ?? root.doc,
  };
  root = reconcileCommand(root, effectiveGlobals(parentGlobals));
  return {
    ...root,
    globals: {
      options: [...parentGlobals.options, ...root.globals.options],
      flags: [...parentGlobals.flags, ...root.globals.flags],
    },
    reconcileInheritedGlobals: true,
  };
}

/** Return a copy of `parent` with the graft root appended as a subcommand. */
export function appendGraftedSubtree(
  parent: ExtendedCommandNode,
  graft: ExtendedCommandNode,
): ExtendedCommandNode {
  return {
    ...parent,
    subcommands: [...parent.subcommands, graft],
  };
}

/**
 * Resolve deferred `value-is` literals against the final recursive-global
 * scope, return a detached normalized tree, and validate all producer rules.
 */
export function normalizeExtendedTool(tool: ExtendedToolType): ExtendedToolType {
  const root = cloneCommandTree(tool.root);
  normalizeCommand(root, new Map(), [], new Set(), new Set());
  const normalized = new ExtendedToolType(tool.version, root);
  validateExtendedTool(normalized);
  return normalized;
}

function normalizeCommand(
  node: MutableCommandNode,
  ancestorScope: ReadonlyMap<string, ValueIsScopeEntry>,
  ancestorGlobals: readonly EffectiveCommandField[],
  visited: Set<MutableCommandNode>,
  onStack: Set<MutableCommandNode>,
): void {
  if (onStack.has(node)) {
    toolBuildError('command-tree-cycle', `the command tree contains a cycle at ${node.name}`);
  }
  if (visited.has(node)) {
    toolBuildError('duplicate-command-parent', `command "${node.name}" has more than one parent`);
  }
  visited.add(node);
  onStack.add(node);
  if (node.reconcileInheritedGlobals) {
    const reconciled = reconcileCommand(node, ancestorGlobals);
    node.globals = reconciled.globals;
    node.body = reconciled.body;
  }
  const scope = new Map(ancestorScope);
  node.globals.options.forEach((option) => registerOption(scope, option));
  node.globals.flags.forEach((flag) => {
    scope.set(flag.long, {});
    flag.aliases.forEach((alias) => scope.set(alias, {}));
  });
  if (node.body) {
    registerBody(scope, node.body);
    node.body = {
      ...node.body,
      constraints: node.body.constraints.map((constraint) => resolveConstraint(constraint, scope)),
    };
  }
  const childGlobals = [...ancestorGlobals, ...effectiveGlobals(node.globals)];
  node.subcommands.forEach((child) =>
    normalizeCommand(child, scope, childGlobals, visited, onStack),
  );
  onStack.delete(node);
}

type ReconciledFieldShape =
  | { readonly tag: 'value'; readonly codec: FluentCodec }
  | { readonly tag: 'bool-flag' }
  | { readonly tag: 'count-flag' };

function reconcileCommand(
  node: MutableCommandNode,
  ancestors: readonly EffectiveCommandField[],
): MutableCommandNode {
  if (ancestors.length === 0) return node;
  const globals = {
    options: node.globals.options.filter(
      (option) =>
        !reconcileLocal(
          [option.long, ...option.aliases],
          { tag: 'value', codec: optionCollectedCodec(option.shape) },
          ancestors,
          node.name,
        ),
    ),
    flags: node.globals.flags.filter(
      (flag) =>
        !reconcileLocal([flag.long, ...flag.aliases], flagShape(flag), ancestors, node.name),
    ),
  };
  const body = node.body
    ? {
        ...node.body,
        positionals: {
          fixed: node.body.positionals.fixed.filter(
            (positional) =>
              !reconcileLocal(
                [positional.name],
                { tag: 'value', codec: positional.codec },
                ancestors,
                node.name,
              ),
          ),
          tail:
            node.body.positionals.tail &&
            !reconcileLocal(
              [node.body.positionals.tail.name],
              tailShape(node.body.positionals.tail),
              ancestors,
              node.name,
            )
              ? node.body.positionals.tail
              : undefined,
        },
        options: node.body.options.filter(
          (option) =>
            !reconcileLocal(
              [option.long, ...option.aliases],
              { tag: 'value', codec: optionCollectedCodec(option.shape) },
              ancestors,
              node.name,
            ),
        ),
        flags: node.body.flags.filter(
          (flag) =>
            !reconcileLocal([flag.long, ...flag.aliases], flagShape(flag), ancestors, node.name),
        ),
      }
    : undefined;
  return { ...node, globals, body };
}

function effectiveGlobals(globals: ExtendedGlobals): EffectiveCommandField[] {
  return [
    ...globals.options.map((option) => ({ tag: 'option' as const, option })),
    ...globals.flags.map((flag) => ({ tag: 'flag' as const, flag })),
  ];
}

function reconcileLocal(
  localNames: readonly string[],
  localShape: ReconciledFieldShape,
  ancestors: readonly EffectiveCommandField[],
  commandName: string,
): boolean {
  const matches = ancestors.filter((ancestor) =>
    localNames.some((name) => inheritedSurfaceNames(ancestor).includes(name)),
  );
  if (matches.length === 0) return false;
  if (matches.length > 1) {
    toolBuildError(
      'inherited-global-ambiguous',
      `parameter surface name "${localNames[0]}" on command "${commandName}" matches multiple inherited globals: ${matches
        .map((match) => `"${inheritedPrimaryName(match)}"`)
        .join(', ')}`,
    );
  }
  const inherited = matches[0];
  if (!fieldShapesCompatible(inheritedShape(inherited), localShape)) {
    const inheritedName = inheritedPrimaryName(inherited);
    const collidingName =
      localNames.find((name) => inheritedSurfaceNames(inherited).includes(name)) ?? localNames[0];
    toolBuildError(
      'inherited-global-incompatible',
      `parameter surface name "${collidingName}" on command "${commandName}" is incompatible with inherited global "${inheritedName}"`,
    );
  }
  return true;
}

function inheritedSurfaceNames(field: EffectiveCommandField): readonly string[] {
  return field.tag === 'option'
    ? [field.option.long, ...field.option.aliases]
    : [field.flag.long, ...field.flag.aliases];
}

function inheritedPrimaryName(field: EffectiveCommandField): string {
  return field.tag === 'option' ? field.option.long : field.flag.long;
}

function inheritedShape(field: EffectiveCommandField): ReconciledFieldShape {
  return field.tag === 'option'
    ? { tag: 'value', codec: optionCollectedCodec(field.option.shape) }
    : flagShape(field.flag);
}

function flagShape(flag: ExtendedGlobals['flags'][number]): ReconciledFieldShape {
  return { tag: flag.shape.tag === 'bool-flag' ? 'bool-flag' : 'count-flag' };
}

function tailShape(tail: ExtendedTailPositional): ReconciledFieldShape {
  return { tag: 'value', codec: listCodec(tail.itemCodec) };
}

function fieldShapesCompatible(left: ReconciledFieldShape, right: ReconciledFieldShape): boolean {
  return left.tag === 'value' && right.tag === 'value'
    ? schemaShapesMatch(left.codec.graph, right.codec.graph)
    : left.tag === right.tag;
}

function registerBody(scope: Map<string, ValueIsScopeEntry>, body: ExtendedCommandBody): void {
  body.options.forEach((option) => registerOption(scope, option));
  body.flags.forEach((flag) => {
    scope.set(flag.long, {});
    flag.aliases.forEach((alias) => scope.set(alias, {}));
  });
  body.positionals.fixed.forEach((positional) =>
    scope.set(positional.name, {
      codec: positional.codec,
      mode: 'whole-or-one-peel',
    }),
  );
  if (body.positionals.tail) {
    scope.set(body.positionals.tail.name, {
      codec: body.positionals.tail.itemCodec,
      mode: 'exact',
    });
  }
}

function registerOption(scope: Map<string, ValueIsScopeEntry>, option: ExtendedOptionSpec): void {
  const entry: ValueIsScopeEntry = {
    codec: optionValueCodec(option.shape),
    mode:
      option.shape.tag === 'scalar' || option.shape.tag === 'optional-scalar'
        ? 'whole-or-one-peel'
        : 'exact',
  };
  scope.set(option.long, entry);
  option.aliases.forEach((alias) => scope.set(alias, entry));
}

function resolveConstraint(
  constraint: ExtendedConstraint,
  scope: ReadonlyMap<string, ValueIsScopeEntry>,
): ExtendedConstraint {
  switch (constraint.tag) {
    case 'requires-all':
    case 'all-or-none':
    case 'requires-any':
      return { ...constraint, refs: constraint.refs.map((ref) => resolveRef(ref, scope)) };
    case 'mutex-groups':
      return {
        ...constraint,
        groups: constraint.groups.map((group) => group.map((ref) => resolveRef(ref, scope))),
      };
    case 'implies':
    case 'forbids':
      return {
        ...constraint,
        lhs: constraint.lhs.map((ref) => resolveRef(ref, scope)),
        rhs: constraint.rhs.map((ref) => resolveRef(ref, scope)),
      };
  }
}

function resolveRef(ref: ExtendedRef, scope: ReadonlyMap<string, ValueIsScopeEntry>): ExtendedRef {
  if (ref.tag === 'present') return ref;
  if (ref.value.tag === 'resolved') {
    return {
      ...ref,
      value: { ...ref.value, schemaValue: cloneSchemaValue(ref.value.schemaValue) },
    };
  }
  if (!scope.has(ref.name)) return ref;
  const entry = scope.get(ref.name);
  if (!entry?.codec || !entry.mode) {
    toolBuildError(
      'value-is-type-mismatch',
      `value-is literal does not match the argument type: ${ref.name}`,
    );
  }
  for (const codec of valueIsCodecs(entry.codec, entry.mode)) {
    const sourceValue = parseSourceValue(codec, ref.value.value);
    if (sourceValue.tag === 'invalid') continue;
    let schemaValue;
    try {
      schemaValue = cloneSchemaValue(codec.toValue(sourceValue.value));
    } catch (error) {
      if (error instanceof CodecShapeMismatchError) continue;
      throw error;
    }
    if (schemaValueConforms(codec.graph, codec.graph.root, schemaValue)) {
      return {
        ...ref,
        value: {
          tag: 'resolved',
          codec,
          value: sourceValue.value,
          schemaValue,
        },
      };
    }
  }
  toolBuildError(
    'value-is-type-mismatch',
    `value-is literal does not match the argument type: ${ref.name}`,
  );
}

interface MutableCommandNode {
  name: string;
  aliases: readonly string[];
  doc: Doc;
  globals: ExtendedGlobals;
  subcommands: MutableCommandNode[];
  body?: ExtendedCommandBody;
  reconcileInheritedGlobals?: true;
}

function cloneCommandTree(root: ExtendedCommandNode): MutableCommandNode {
  const clones = new Map<ExtendedCommandNode, MutableCommandNode>();
  const clone = (node: ExtendedCommandNode): MutableCommandNode => {
    const existing = clones.get(node);
    if (existing) return existing;
    const result: MutableCommandNode = {
      name: node.name,
      aliases: [...node.aliases],
      doc: node.doc,
      globals: {
        options: [...node.globals.options],
        flags: [...node.globals.flags],
      },
      subcommands: [],
      reconcileInheritedGlobals: node.reconcileInheritedGlobals,
      body: node.body
        ? {
            ...node.body,
            positionals: {
              fixed: [...node.body.positionals.fixed],
              tail: node.body.positionals.tail,
            },
            options: [...node.body.options],
            flags: [...node.body.flags],
            constraints: [...node.body.constraints],
            errors: [...node.body.errors],
          }
        : undefined,
    };
    clones.set(node, result);
    result.subcommands = node.subcommands.map(clone);
    return result;
  };
  return clone(root);
}
