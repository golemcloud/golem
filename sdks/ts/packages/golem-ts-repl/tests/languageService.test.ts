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
