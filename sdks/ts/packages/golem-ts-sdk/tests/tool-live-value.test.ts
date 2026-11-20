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
import { z } from 'zod/v4';
import { toolDefinition } from '../src/index';
import { Caller } from '../src/caller';
import { LIVE_VALUE_MESSAGE } from '../src/liveValues';
import { encodeToolValue, encodeToolValueAsync } from '../src/internal/tool/invocationResult';
import { compileSchema } from '../src/schema/adapter';

function errorOf(run: () => unknown): string {
  try {
    run();
    return '<no error>';
  } catch (error) {
    return error instanceof Error
      ? error.message
      : typeof error === 'object' && error !== null && 'val' in error
        ? String(error.val)
        : String(error);
  }
}

describe('a live value in a tool', () => {
  it('is refused with the shared message when it is a tool option default', () => {
    const message = errorOf(() =>
      toolDefinition('option-tool').body((b) =>
        b.option('who', z.string(), { default: Caller.anonymous() as never }).returns(z.void()),
      ),
    );
    expect(message).toContain(LIVE_VALUE_MESSAGE);
  });

  it('is refused with the shared message when it is a positional default', () => {
    const message = errorOf(() =>
      toolDefinition('positional-tool').body((b) =>
        b
          .positional('who', z.string(), { default: Caller.anonymous() as never, required: false })
          .returns(z.void()),
      ),
    );
    expect(message).toContain(LIVE_VALUE_MESSAGE);
  });

  it('is refused with the shared message when a handler returns it', async () => {
    const codec = compileSchema(z.object({ nested: z.array(z.string()) }));
    expect(
      errorOf(() => encodeToolValue(codec, { nested: [Caller.anonymous()] }, 'tool result')),
    ).toContain(LIVE_VALUE_MESSAGE);
    await expect(
      encodeToolValueAsync(codec, { nested: [Caller.anonymous()] }, 'tool result'),
    ).rejects.toMatchObject({
      tag: 'invalid-result',
      val: expect.stringContaining(LIVE_VALUE_MESSAGE),
    });
  });
});
