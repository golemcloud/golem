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

// Projection of the production-mapped `ResolvedGraph` (as stored in the registry
// by the `@agent` decorator) into the new schema model via
// `resolvedGraphToSchemaType`: named-def registration, built-in generic
// inlining, and the structural shapes produced for representative TypeScript
// types. Builder-level dedup / conflict / merge semantics are covered by the
// Slice-1 `tests/schema-model/edge-cases.test.ts`; recursive-type projection is
// covered by `tests/schema-mapping/recursion.test.ts`.

import { describe, it, expect } from 'vitest';
import { buildTypeFromJSON, LiteTypeJSON } from '@golemcloud/golem-ts-types-core';
import {
  getTestObjectType,
  getTestMapType,
  getTestListOfObjectType,
  getUnionWithOnlyLiterals,
  getResultTypeExact,
  fetchTypeFromBarAgent,
} from '../testUtils';
import { SchemaType, SchemaTypeDef } from '../../src/internal/schema-model';
import { project } from './helpers';
import * as Either from '../../src/newTypes/either';
import { mapTsTypeToResolvedGraph } from '../../src/internal/mapping/types/resolvedMapper';
import { resolvedGraphToSchemaType } from '../../src/internal/mapping/types/schemaType';
import { TypeScope } from '../../src/internal/mapping/types/scope';

function def(mapping: { graph: { defs: Map<string, SchemaTypeDef> } }, id: string): SchemaType {
  const d = mapping.graph.defs.get(id);
  if (!d) throw new Error(`missing def ${id}`);
  return d.body;
}

describe('Type projection (resolvedGraphToSchemaType)', () => {
  it('named record becomes a ref to a single registered def', () => {
    const [graph] = getTestObjectType();
    const mapping = project(graph);

    expect(mapping.root.body.tag).toBe('ref');
    expect(mapping.graph.defs.size).toBeGreaterThanOrEqual(1);
    if (mapping.root.body.tag === 'ref') {
      const body = def(mapping, mapping.root.body.id);
      expect(body.body.tag).toBe('record');
    }
  });

  it('map projects to a structural map node (not a named def)', () => {
    const [graph] = getTestMapType();
    const mapping = project(graph);
    expect(mapping.root.body.tag).toBe('map');
    if (mapping.root.body.tag === 'map') {
      expect(mapping.root.body.key.body.tag).toBe('string');
    }
  });

  it('list of named objects projects to list-of-ref', () => {
    const [graph] = getTestListOfObjectType();
    const mapping = project(graph);
    expect(mapping.root.body.tag).toBe('list');
    if (mapping.root.body.tag === 'list') {
      expect(mapping.root.body.element.body.tag).toBe('ref');
    }
  });

  it('union of only literals projects to an enum def', () => {
    const [graph] = getUnionWithOnlyLiterals();
    const mapping = project(graph);
    // Named enum -> ref to enum def; anonymous -> inline enum.
    const body =
      mapping.root.body.tag === 'ref' ? def(mapping, mapping.root.body.id) : mapping.root;
    expect(body.body.tag).toBe('enum');
    if (body.body.tag === 'enum') {
      expect(body.body.cases).toEqual(['foo', 'bar', 'baz']);
    }
  });

  it('tagged union projects to a variant def with named cases', () => {
    const [graph] = fetchTypeFromBarAgent('TaggedUnion');
    const mapping = project(graph);
    const body =
      mapping.root.body.tag === 'ref' ? def(mapping, mapping.root.body.id) : mapping.root;
    expect(body.body.tag).toBe('variant');
  });

  it('inbuilt result projects to a structural result node', () => {
    const [graph] = getResultTypeExact();
    const mapping = project(graph);
    expect(mapping.root.body.tag).toBe('result');
    if (mapping.root.body.tag === 'result') {
      expect(mapping.root.body.ok?.body.tag).toBe('f64');
      expect(mapping.root.body.err?.body.tag).toBe('string');
    }
  });

  it('optional Secret<T> makes the handle optional without making the revealed payload optional', () => {
    const secretType: LiteTypeJSON = {
      kind: 'secret',
      optional: false,
      typeArg: { kind: 'string', optional: false },
    };
    const result = mapTsTypeToResolvedGraph(
      buildTypeFromJSON(secretType),
      TypeScope.method('Agent.method', 'secret', true),
    );
    const graph = Either.getOrThrowWith(result, (err) => new Error(`mapping failed: ${err}`));
    const mapping = resolvedGraphToSchemaType(graph);

    expect(mapping.root.body.tag).toBe('option');
    if (mapping.root.body.tag !== 'option') return;
    expect(mapping.root.body.element.body.tag).toBe('secret');
    if (mapping.root.body.element.body.tag !== 'secret') return;
    expect(mapping.root.body.element.body.inner.body.tag).toBe('string');
  });
});
