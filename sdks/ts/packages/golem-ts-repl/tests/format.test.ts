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
import util from 'node:util';
import { GolemServiceError } from '@golemcloud/golem-ts-bridge';
import {
  formatEvalError,
  installGolemServiceErrorInspect,
  wrapSnippetInfoLine,
} from '../src/format';

describe('wrapSnippetInfoLine', () => {
  it('does not wrap short callable signatures', () => {
    const line = '(x: number): Promise<number>;';

    expect(wrapSnippetInfoLine(line, 120)).toEqual([line]);
  });

  it('wraps long callable signatures by top-level parameters', () => {
    expect(
      wrapSnippetInfoLine(
        '(x: number, y: string, z: { tag: "case1"; val: number; } | { tag: "case2"; val: string; }): Promise<number>;',
        50,
      ),
    ).toEqual([
      '(',
      '  x: number,',
      '  y: string,',
      '  z: { tag: "case1"; val: number; }',
      '    | { tag: "case2"; val: string; }',
      '): Promise<number>;',
    ]);
  });

  it('wraps long property callable signatures', () => {
    expect(
      wrapSnippetInfoLine('schedule: (scheduleAt: string, x: number, y: string) => void;', 40),
    ).toEqual([
      'schedule: (',
      '  scheduleAt: string,',
      '  x: number,',
      '  y: string',
      ') => void;',
    ]);
  });

  it('does not split generic type arguments', () => {
    expect(
      wrapSnippetInfoLine(
        '(x: Map<string, number>, y: Array<{ a: string, b: number }>): void;',
        40,
      ),
    ).toEqual([
      '(',
      '  x: Map<string, number>,',
      '  y: Array<{ a: string, b: number }>',
      '): void;',
    ]);
  });

  it('does not split commas inside string literal types', () => {
    expect(wrapSnippetInfoLine('(x: "a,b", y: string): void;', 20)).toEqual([
      '(',
      '  x: "a,b",',
      '  y: string',
      '): void;',
    ]);
  });

  it('does not split unions inside string literal types', () => {
    expect(wrapSnippetInfoLine('(x: "ok|failed" | "unknown", y: string): void;', 24)).toEqual([
      '(',
      '  x: "ok|failed"',
      '    | "unknown",',
      '  y: string',
      '): void;',
    ]);
  });

  it('preserves base indentation when wrapping', () => {
    expect(
      wrapSnippetInfoLine(
        '  abortable: (signal: AbortSignal, x: number, y: string) => Promise<number>;',
        48,
      ),
    ).toEqual([
      '  abortable: (',
      '    signal: AbortSignal,',
      '    x: number,',
      '    y: string',
      '  ) => Promise<number>;',
    ]);
  });
});

describe('formatEvalError', () => {
  it('formats structured Golem worker errors without raw JSON', () => {
    const error = new GolemServiceError({
      operation: 'invokeAgent',
      status: 500,
      statusText: 'Internal Server Error',
      bodyText: JSON.stringify({
        code: 'INTERNAL_AGENT_EXECUTION_FAILED',
        error: 'Invocation Failed',
        workerError: {
          cause:
            'error while executing at wasm backtrace:\n    0: 0x56570e - agent_guest.wasm!abort\n    1: 0x1e09ed - agent_guest.wasm!golem:agent/guest@1.5.0#invoke: wasm trap: wasm `unreachable` instruction executed',
          stderr:
            "\nthread '<unnamed>' (1) panicked at src/internal.rs:2106:41:\nException during awaiting call result for guest.invoke:\nJavaScript error: BOOM!\nStack:\n    at boom (user:91:19)\n    at invoke (@golemcloud/golem-ts-sdk:2:75626)\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n",
        },
      }),
      body: {
        code: 'INTERNAL_AGENT_EXECUTION_FAILED',
        error: 'Invocation Failed',
        agentError: {
          cause:
            'error while executing at wasm backtrace:\n    0: 0x56570e - agent_guest.wasm!abort\n    1: 0x1e09ed - agent_guest.wasm!golem:agent/guest@1.5.0#invoke: wasm trap: wasm `unreachable` instruction executed',
          stderr:
            "\nthread '<unnamed>' (1) panicked at src/internal.rs:2106:41:\nException during awaiting call result for guest.invoke:\nJavaScript error: BOOM!\nStack:\n    at boom (user:91:19)\n    at invoke (@golemcloud/golem-ts-sdk:2:75626)\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n",
        },
      },
    });
    error.stack = `${error.message}\n    at invokeAgent (bridge/index.mjs:139:23)\n    at async REPL1:1:33`;

    const formatted = util.stripVTControlCharacters(formatEvalError(error));

    expect(formatted).toContain('Service response:');
    expect(formatted).toContain('Status: 500 Internal Server Error');
    expect(formatted).toContain('Code: INTERNAL_AGENT_EXECUTION_FAILED');
    expect(formatted).toContain('Message: Invocation Failed');
    expect(formatted).toContain('Stderr:');
    expect(formatted).toContain('JavaScript error: BOOM!');
    expect(formatted).toContain('at boom (user:91:19)');
    expect(formatted).toContain('Cause:');
    expect(formatted).toContain('agent_guest.wasm!abort');
    expect(formatted).toContain('wasm trap: wasm `unreachable` instruction executed');
    expect(formatted).toContain('Bridge stack:');
    expect(formatted).not.toContain('{"code"');
    expect(formatted).not.toContain('Worker stderr:');
    expect(formatted).not.toContain('Worker cause:');

    expect(error.message).toContain('Message: Invocation Failed');
    expect(error.message).toContain('Stderr:');
    expect(error.message).toContain('JavaScript error: BOOM!');
    expect(error.message).toContain('Wasm trap: wasm `unreachable` instruction executed');
    expect(error.message).not.toContain('{"code"');
    expect(error.message).not.toContain('agent_guest.wasm!abort');
  });

  it('keeps ordinary JavaScript errors unchanged', () => {
    const error = new Error('plain failure');
    error.stack = 'Error: plain failure\n    at user.ts:1:1';

    expect(formatEvalError(error)).toBe(error.stack);
  });

  it('does not expose structured Golem error fields to object inspection', () => {
    const error = new GolemServiceError({
      operation: 'invokeAgent',
      status: 500,
      statusText: 'Internal Server Error',
      bodyText: '{"code":"INTERNAL_AGENT_EXECUTION_FAILED"}',
      body: { code: 'INTERNAL_AGENT_EXECUTION_FAILED', error: 'Invocation Failed' },
    });

    expect(Object.keys(error)).toEqual([]);
  });

  it('uses compact structured output for Node object inspection', () => {
    installGolemServiceErrorInspect();

    const error = new GolemServiceError({
      operation: 'invokeAgent',
      status: 500,
      statusText: 'Internal Server Error',
      bodyText: '{"code":"INTERNAL_AGENT_EXECUTION_FAILED"}',
      body: {
        code: 'INTERNAL_AGENT_EXECUTION_FAILED',
        error: 'Invocation Failed',
        agentError: {
          cause:
            'error while executing at wasm backtrace:\n    0: 0x56570e - agent_guest.wasm!abort\n    1: 0x1e09ed - agent_guest.wasm!golem:agent/guest@1.5.0#invoke: wasm trap: wasm `unreachable` instruction executed',
          stderr: 'JavaScript error: BOOM!\nStack:\n    at boom (user:91:19)',
        },
      },
    });
    error.stack = `${error.message}\n    at invokeAgent (bridge/index.mjs:139:23)`;

    const inspected = util.stripVTControlCharacters(util.inspect(error));

    expect(inspected).toContain('Service response:');
    expect(inspected).toContain('Status: 500 Internal Server Error');
    expect(inspected).toContain('Message: Invocation Failed');
    expect(inspected).toContain('JavaScript error: BOOM!');
    expect(inspected).toContain('Cause:');
    expect(inspected).toContain('Bridge stack:');
    expect(inspected).not.toContain('bodyText');
    expect(inspected).not.toContain('{"code"');
    expect(inspected).toContain('agent_guest.wasm!abort');
    expect(inspected).not.toContain('Worker cause:');
  });

  it('keeps rust-style cause frames highlighted in formatted output', () => {
    const error = new GolemServiceError({
      operation: 'invokeAgent',
      status: 500,
      statusText: 'Internal Server Error',
      body: {
        code: 'INTERNAL_AGENT_EXECUTION_FAILED',
        error: 'Invocation Failed',
        agentError: {
          stderr: "thread '<unnamed>' panicked at src/counter_agent.rs:36:9:\nBOOM!",
          cause:
            'error while executing at wasm backtrace:\n   11:   0xcad3 - <xyz_rust_main::counter_agent::CounterImpl>::boom\n                    at /tmp/src/counter_agent.rs:36:9: wasm trap: wasm `unreachable` instruction executed',
        },
      },
    });

    const formatted = formatEvalError(error);
    const stripped = util.stripVTControlCharacters(formatted);

    expect(stripped).toContain('<xyz_rust_main::counter_agent::CounterImpl>::boom');
    expect(stripped).toContain('/tmp/src/counter_agent.rs:36:9');
  });

  it('formats non-json proxy response bodies in base and inspect output', () => {
    installGolemServiceErrorInspect();

    const error = new GolemServiceError({
      operation: 'invokeAgent',
      status: 502,
      statusText: 'Bad Gateway',
      bodyText: '<html>Bad Gateway</html>',
    });

    expect(error.message).toContain('Agent invocation failed: 502 Bad Gateway');
    expect(error.message).toContain('Response body:');
    expect(error.message).toContain('<html>Bad Gateway</html>');

    const inspected = util.stripVTControlCharacters(util.inspect(error));
    expect(inspected).toContain('Service response:');
    expect(inspected).toContain('Status: 502 Bad Gateway');
    expect(inspected).toContain('<html>Bad Gateway</html>');
    expect(inspected).not.toContain('bodyText');
  });

  it('formats unknown json response message fields without raw json', () => {
    const error = new GolemServiceError({
      operation: 'invokeAgent',
      status: 502,
      statusText: 'Bad Gateway',
      bodyText: JSON.stringify({ message: 'upstream unavailable' }),
    });

    expect(error.message).toContain('Agent invocation failed: 502 Bad Gateway');
    expect(error.message).toContain('Response message: upstream unavailable');
    expect(error.message).not.toContain('{"message"');
  });

  it('formats unknown json response error arrays without raw json', () => {
    const error = new GolemServiceError({
      operation: 'createAgent',
      status: 502,
      statusText: 'Bad Gateway',
      bodyText: JSON.stringify({ errors: ['proxy failed', 'upstream timeout'] }),
    });

    expect(error.message).toContain('Agent creation failed: 502 Bad Gateway');
    expect(error.message).toContain('Response messages:');
    expect(error.message).toContain('- proxy failed');
    expect(error.message).toContain('- upstream timeout');
    expect(error.message).not.toContain('{"errors"');
  });

  it('pretty prints unknown json response bodies as fallback', () => {
    const error = new GolemServiceError({
      operation: 'invokeAgent',
      status: 502,
      statusText: 'Bad Gateway',
      bodyText: JSON.stringify({ foo: 'bar' }),
    });

    expect(error.message).toContain('Response body:');
    expect(error.message).toContain(`{
  "foo": "bar"
}`);
  });
});
