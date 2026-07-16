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

import { describe, it, expect } from 'vitest';
import { z } from 'zod';
import { compileSchema } from '../src/fluent/schema/adapter';
import { s } from '../src/fluent/schema/markers';
import { Result } from '../src/host/result';

describe('fluent s.result → WIT result<ok, err>', () => {
  const NotFound = z.object({ _tag: z.literal('NotFoundError'), resource: z.string() });
  const codec = () => compileSchema(s.result(z.number(), NotFound));

  it('lowers to a result type node with ok / err sub-types', () => {
    const body = codec().graph.root.body as {
      tag: string;
      ok: { body: { tag: string } };
      err: { body: { tag: string } };
    };
    expect(body.tag).toBe('result');
    expect(body.ok.body.tag).toBe('f64');
    expect(body.err.body.tag).toBe('record');
  });

  it('encodes Result.ok to a result/ok value', () => {
    expect(codec().toValue(Result.ok(7))).toEqual({
      tag: 'result',
      result: { tag: 'ok', value: { tag: 'f64', value: 7 } },
    });
  });

  it('encodes Result.err to a result/err value', () => {
    const enc = codec().toValue(Result.err({ _tag: 'NotFoundError', resource: 'x' })) as {
      tag: string;
      result: { tag: string };
    };
    expect(enc.tag).toBe('result');
    expect(enc.result.tag).toBe('err');
  });

  it('round-trips ok and err back to a Result', () => {
    const c = codec();

    const okDec = c.fromValue(c.toValue(Result.ok(7))) as Result<number, unknown>;
    expect(okDec.isOk()).toBe(true);
    expect(okDec.val).toBe(7);

    const errVal = { _tag: 'NotFoundError' as const, resource: 'x' };
    const errDec = c.fromValue(c.toValue(Result.err(errVal))) as Result<unknown, typeof errVal>;
    expect(errDec.isErr()).toBe(true);
    expect(errDec.val).toEqual(errVal);
  });
});
