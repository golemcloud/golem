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
import { describe, expect, it } from 'vitest';
import vectors from '../../../../../test-data/caller-identity/text-form-v1.json';
import { PrincipalRef } from '../src/caller';
import { compileSchema } from '../src/schema/adapter';
import { s } from '../src/schema/markers';
import type { SchemaValue } from '../src/internal/schema-model';

describe('PrincipalRef text form', () => {
  it('prints and parses exactly the shared vectors', () => {
    expect(vectors.roundTrip.length).toBeGreaterThan(0);
    for (const vector of vectors.roundTrip) {
      const ref = new PrincipalRef(vector.issuer, vector.subject);
      expect(ref.toString(), vector.name).toBe(vector.text);
      const parsed = PrincipalRef.parse(vector.text);
      expect(parsed.issuer, vector.name).toBe(vector.issuer);
      expect(parsed.subject, vector.name).toBe(vector.subject);
    }
  });

  it('refuses every non-canonical spelling', () => {
    expect(vectors.reject.length).toBeGreaterThan(0);
    for (const vector of vectors.reject) {
      expect(() => PrincipalRef.parse(vector.text), vector.name).toThrow(SyntaxError);
    }
  });

  it('compares both strings byte for byte', () => {
    const base = new PrincipalRef('https://login.example/', 'Subject-1');
    expect(base.equals(new PrincipalRef('https://login.example/', 'Subject-1'))).toBe(true);
    expect(base.equals(new PrincipalRef('https://login.example', 'Subject-1'))).toBe(false);
    expect(base.equals(new PrincipalRef('https://login.example/', 'subject-1'))).toBe(false);
    expect(base.equals(new PrincipalRef('https://login.example/', 'Subject-1 '))).toBe(false);
  });
});

describe('s.principalRef()', () => {
  it('is the named record principal-ref { issuer, subject }', () => {
    const codec = compileSchema(s.principalRef());
    expect(codec.graph.root.body).toEqual({ tag: 'ref', id: 'principal-ref' });
    const definition = codec.graph.defs.get('principal-ref' as never);
    const body = definition?.body.body as { tag: string; fields: { name: string }[] };
    expect(body.tag).toBe('record');
    expect(body.fields.map((field) => field.name)).toEqual(['issuer', 'subject']);
  });

  it('round-trips as a parameter value', () => {
    const codec = compileSchema(s.principalRef());
    const original = new PrincipalRef('https://login.example', 'u-1');
    const wire = codec.toValue(original) as Extract<SchemaValue, { tag: 'record' }>;
    expect(wire.tag).toBe('record');
    expect(wire.fields.map((field) => (field as { value: string }).value)).toEqual([
      'https://login.example',
      'u-1',
    ]);
    const decoded = codec.fromValue(wire) as PrincipalRef;
    expect(decoded).toBeInstanceOf(PrincipalRef);
    expect(decoded.equals(original)).toBe(true);
  });

  it('is accepted in an agent definition as a parameter, a result and a nested field', async () => {
    const { defineAgent } = await import('../src/defineAgent');
    const { method } = await import('../src/method');
    const { z } = await import('zod');
    expect(() =>
      defineAgent({
        name: 'PrincipalRefAgent',
        id: { owner: s.principalRef() },
        methods: {
          echo: method({ input: { who: s.principalRef() }, returns: s.principalRef() }),
          nested: method({
            input: { data: z.object({ who: s.principalRef() }) },
            returns: z.array(s.principalRef()),
          }),
        },
      }),
    ).not.toThrow();
  });
});
