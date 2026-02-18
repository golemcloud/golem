// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import util from 'node:util';
import pc from 'picocolors';
import { getTerminalWidth, writeln } from './process';

export const INFO_PREFIX = pc.bold(pc.red('>'));
export const INFO_PREFIX_LENGTH = util.stripVTControlCharacters(INFO_PREFIX).length + 1;

export function logSnippetInfo(message: string | string[]) {
  const lines = Array.isArray(message) ? message : message.split('\n');
  if (lines.length === 0) return;

  let maxLineLength = 0;
  lines.forEach((line) => {
    maxLineLength = Math.max(maxLineLength, util.stripVTControlCharacters(line).length);
    writeln(`${INFO_PREFIX} ${line}`);
  });

  if (maxLineLength > 0) {
    writeFullLineSeparator();
  }
}

export function formatAsTable(items: string[]): string {
  if (items.length === 0) return '';

  const maxLineLength = getTerminalWidth() - INFO_PREFIX_LENGTH;
  const separator = '  ';

  const colWidth = Math.max(...items.map((s) => s.length));
  const sepLen = separator.length;

  const cols = Math.max(1, Math.floor((maxLineLength + sepLen) / (colWidth + sepLen)));

  const lines: string[] = [];
  let line: string[] = [];

  for (const item of items) {
    if (item.length > maxLineLength) {
      if (line.length) {
        lines.push(line.join(separator).trimEnd());
        line = [];
      }
      lines.push(item);
      continue;
    }

    line.push(item.padEnd(colWidth, ' '));
    if (line.length === cols) {
      lines.push(line.join(separator).trimEnd());
      line = [];
    }
  }

  if (line.length) {
    lines.push(line.join(separator).trimEnd());
  }

  return lines.join('\n');
}

export function formatEvalError(error: unknown) {
  if (error instanceof Error) {
    return error.stack ?? error.message;
  }
  return String(error);
}

export function writeFullLineSeparator() {
  const width = getTerminalWidth();
  if (!width || width <= 0) return;
  writeln(pc.dim('~'.repeat(width)));
}
