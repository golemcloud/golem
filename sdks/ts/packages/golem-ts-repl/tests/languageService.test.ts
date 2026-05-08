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
import { LanguageService } from '../src/language-service';
import type { Config, ReplCliFlags } from '../src/config';

const config: Config = {
  binary: '',
  appMainDir: '',
  agents: {},
  historyFile: '',
  cliCommandsMetadataJsonPath: '',
};

const flags: ReplCliFlags = {
  disableAutoImports: true,
  showTypeInfo: false,
  streamLogs: false,
};

function getSnippetCompletionEntries(history: string, snippet: string): string[] {
  const languageService = new LanguageService(config, flags);
  languageService.addSnippetToHistory(history);
  languageService.setSnippet(snippet);
  return languageService.getSnippetCompletions()?.entries ?? [];
}

function typeCheckSnippet(history: string, snippet: string, customConfig = config) {
  const languageService = new LanguageService(customConfig, flags);
  languageService.addSnippetToHistory(history);
  languageService.setSnippet(snippet);
  const result = languageService.typeCheckSnippet();
  expect(result.ok).toBe(false);
  return result.ok ? undefined : result;
}

const remoteMethodHistory = [
  'type CancellationToken = string;',
  'type RemoteMethod<Args extends unknown[], R> = {',
  '  (...args: Args): Promise<R>;',
  '  abortable: (signal: AbortSignal, ...args: Args) => Promise<R>;',
  '  trigger: (...args: Args) => void;',
  '  schedule: (ts: string, ...args: Args) => void;',
  '  scheduleCancelable: (ts: string, ...args: Args) => CancellationToken;',
  '};',
  'declare const MyAgent: { get: RemoteMethod<[string], string> };',
  'declare function plain(value: string): void;',
].join('\n');

const remoteMethodConfig: Config = {
  ...config,
  agents: {
    MyAgent: {
      clientPackageName: '',
      clientPackageImportedName: '',
      package: {},
      mode: 'durable',
      methodParameterNames: {
        get: ['authorization'],
      },
    },
  },
};

function expectRemoteMethodHint(result: { formattedHints: string[] }, expected: string): void {
  expect(result.formattedHints).toHaveLength(1);
  expect(result.formattedHints[0]).toContain(expected);
  expect(result.formattedHints[0]).toContain('authorization');
}

describe('LanguageService tagged union placeholders', () => {
  it('matches runtime tagged-union semantics for tag-based unions', () => {
    const entries = getSnippetCompletionEntries(
      [
        "type Tagged = { text: string; tag: 'text' } | { count: number; tag: 'count' };",
        'declare function takesTagged(value: Tagged): void;',
      ].join('\n'),
      'takesTagged(',
    );

    expect(entries).toEqual(['{ tag: "text", text: "?" }', '{ tag: "count", count: 0 }']);
  });

  it('does not treat non-tag literal properties as runtime tagged unions', () => {
    const entries = getSnippetCompletionEntries(
      [
        "type ByKind = { text: string; kind: 'text' } | { count: number; kind: 'count' };",
        'declare function takesByKind(value: ByKind): void;',
      ].join('\n'),
      'takesByKind(',
    );

    expect(entries).toEqual(['{ text: "?", kind: "text" }', '{ count: 0, kind: "count" }']);
  });
});

describe('LanguageService remote method diagnostic hints', () => {
  it('enriches missing argument diagnostics for remote method invocation', () => {
    const result = typeCheckSnippet(remoteMethodHistory, 'MyAgent.get()', remoteMethodConfig);

    expect(result?.formattedErrors).toContain('TS2554');
    expectRemoteMethodHint(result!, '(authorization: string): Promise<string>;');
  });

  it('enriches wrong argument diagnostics for remote method invocation', () => {
    const result = typeCheckSnippet(remoteMethodHistory, 'MyAgent.get(123)', remoteMethodConfig);

    expect(result?.formattedErrors).toContain('TS2345');
    expectRemoteMethodHint(result!, '(authorization: string): Promise<string>;');
  });

  it('enriches operation diagnostics with the relevant operation signature', () => {
    const cases = [
      ['MyAgent.get.trigger()', 'trigger: (authorization: string) => void;'],
      [
        'MyAgent.get.schedule("2026-01-01T00:00:00Z")',
        'schedule: (scheduleAt: string, authorization: string) => void;',
      ],
      [
        'MyAgent.get.scheduleCancelable("2026-01-01T00:00:00Z")',
        'scheduleCancelable: (scheduleAt: string, authorization: string) => string;',
      ],
      [
        'MyAgent.get.abortable(new AbortController().signal)',
        'abortable: (signal: AbortSignal, authorization: string) => Promise<string>;',
      ],
    ];

    for (const [snippet, expected] of cases) {
      const result = typeCheckSnippet(remoteMethodHistory, snippet, remoteMethodConfig);

      expect(result?.formattedErrors).toContain('TS2554');
      expectRemoteMethodHint(result!, expected);
    }
  });

  it('enriches element-access remote method diagnostics', () => {
    const result = typeCheckSnippet(remoteMethodHistory, 'MyAgent["get"]()', remoteMethodConfig);

    expect(result?.formattedErrors).toContain('TS2554');
    expectRemoteMethodHint(result!, '(authorization: string): Promise<string>;');
  });

  it('enriches unfinished remote invocation diagnostics', () => {
    for (const snippet of ['MyAgent.get(', 'MyAgent.get(1,', 'MyAgent.get(1, 2,']) {
      const result = typeCheckSnippet(remoteMethodHistory, snippet, remoteMethodConfig);

      expect(result?.formattedErrors).toContain('TS1005');
      expectRemoteMethodHint(result!, '(authorization: string): Promise<string>;');
    }
  });

  it('enriches unfinished remote operation diagnostics', () => {
    const cases = [
      ['MyAgent.get.trigger(', 'trigger: (authorization: string) => void;'],
      ['MyAgent.get.schedule(', 'schedule: (scheduleAt: string, authorization: string) => void;'],
      [
        'MyAgent.get.schedule("2026-01-01T00:00:00Z",',
        'schedule: (scheduleAt: string, authorization: string) => void;',
      ],
      [
        'MyAgent.get.scheduleCancelable(',
        'scheduleCancelable: (scheduleAt: string, authorization: string) => string;',
      ],
      [
        'MyAgent.get.abortable(',
        'abortable: (signal: AbortSignal, authorization: string) => Promise<string>;',
      ],
    ];

    for (const [snippet, expected] of cases) {
      const result = typeCheckSnippet(remoteMethodHistory, snippet, remoteMethodConfig);

      expect(result?.formattedErrors).toContain('TS1005');
      expectRemoteMethodHint(result!, expected);
    }
  });

  it('does not enrich normal function diagnostics', () => {
    const result = typeCheckSnippet(remoteMethodHistory, 'plain()', remoteMethodConfig);

    expect(result?.formattedErrors).toContain('TS2554');
    expect(result?.formattedHints).toEqual([]);
  });
});
