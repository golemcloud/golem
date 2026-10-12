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

// Type-only tests for unstructured outputs under `s.option` and `s.result`.
// Checked via `tsc --noEmit`; NOT executed by vitest (`.test-d.ts` suffix).

import { z } from 'zod';
import { defineAgent } from '../src/defineAgent';
import { method } from '../src/method';
import { Result } from '../src/host/result';
import { s, type BinaryReferenceValue, type TextReferenceValue } from '../src/schema/markers';
import type { StandardSchemaV1 } from '../src/schema/standardSchema';

type AssertEqual<X, Y> =
  (<T>() => T extends X ? 1 : 2) extends <T>() => T extends Y ? 1 : 2 ? true : false;

const Pending = z.object({ tag: z.literal('pending') });

const maybe = s.option(s.unstructuredBinary());
const lookup = s.result(s.option(s.unstructuredText()), Pending);

const _maybe: AssertEqual<
  StandardSchemaV1.InferOutput<typeof maybe>,
  BinaryReferenceValue | undefined
> = true;
const _lookup: AssertEqual<
  StandardSchemaV1.InferOutput<typeof lookup>,
  Result<TextReferenceValue | undefined, { tag: 'pending' }>
> = true;
void [_maybe, _lookup];

const Downloads = defineAgent({
  name: 'Downloads',
  id: {},
  methods: {
    raw: method({ input: {}, returns: s.result(s.unstructuredBinary(), Pending) }),
    maybe: method({ input: {}, returns: maybe }),
    lookup: method({ input: {}, returns: lookup }),
  },
});

void Downloads.implement({
  init: () => ({}),
  methods: {
    raw: () => Result.ok({ tag: 'url', val: 'https://example.com' }),
    maybe: () => undefined,
    lookup: () => Result.err({ tag: 'pending' }),
  },
});

void Downloads.implement({
  init: () => ({}),
  methods: {
    raw: () => Result.ok({ tag: 'url', val: 'https://example.com' }),
    // @ts-expect-error an optional unstructured output is a reference or undefined
    maybe: () => 'raw',
    lookup: () => Result.ok({ tag: 'inline', val: 'ready' }),
  },
});

async function call() {
  const remote = Downloads.client.get({});
  const looked = await remote.lookup();
  const found: TextReferenceValue | undefined = looked.isOk() ? looked.val : undefined;
  const maybeBytes: BinaryReferenceValue | undefined = await remote.maybe();
  return [found, maybeBytes];
}
void call;
