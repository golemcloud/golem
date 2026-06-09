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

import util from 'node:util';
import { GolemServiceError, type GolemAgentErrorDetails } from '@golemcloud/golem-ts-bridge';
import pc from 'picocolors';
import { getTerminalWidth, writeln } from './process';

const nodeInspectCustom = Symbol.for('nodejs.util.inspect.custom');
export const INFO_PREFIX = pc.bold(pc.red('>'));
export const INFO_PREFIX_LENGTH = util.stripVTControlCharacters(INFO_PREFIX).length + 1;

export function installGolemServiceErrorInspect() {
  Object.defineProperty(GolemServiceError.prototype, nodeInspectCustom, {
    value(this: GolemServiceError) {
      return formatGolemServiceError({
        message: this.message,
        stack: this.stack,
        status: this.status,
        statusText: this.statusText,
        ...getServiceResponseFields(this),
        agentError: getAgentError(this),
      });
    },
    enumerable: false,
    configurable: true,
  });
}

export function logSnippetInfo(message: string | string[]) {
  const availableLineLength = getTerminalWidth() - INFO_PREFIX_LENGTH;
  const lines = (Array.isArray(message) ? message : message.split('\n')).flatMap((line) =>
    wrapSnippetInfoLine(line, availableLineLength),
  );
  if (lines.length === 0) return;

  let maxLineLength = 0;
  lines.forEach((line) => {
    maxLineLength = Math.max(maxLineLength, util.stripVTControlCharacters(line).length);
    writeln(pc.reset(`${INFO_PREFIX} ${line}`));
  });

  if (maxLineLength > 0) {
    writeFullLineSeparator();
  }
}

export function wrapSnippetInfoLine(line: string, maxLineLength: number): string[] {
  if (maxLineLength <= 0 || visibleLength(line) <= maxLineLength) return [line];

  const signature = findCallableSignature(line);
  if (!signature) return [line];

  const params = splitTopLevelParams(line.slice(signature.paramsStart + 1, signature.paramsEnd));
  if (params.length <= 1) return [line];

  const baseIndent = line.match(/^\s*/)?.[0] ?? '';
  const prefix = line.slice(0, signature.paramsStart + 1);
  const suffix = line.slice(signature.paramsEnd);
  const firstLine = trimVisibleEnd(prefix);
  const paramLines = params.flatMap((param, index) => {
    const comma = index === params.length - 1 ? '' : ',';
    return wrapParamLine(`${baseIndent}  ${trimVisible(param)}${comma}`, maxLineLength);
  });

  return [firstLine, ...paramLines, `${baseIndent}${trimVisibleStart(suffix)}`];
}

function visibleLength(value: string): number {
  return util.stripVTControlCharacters(value).length;
}

function findCallableSignature(
  line: string,
): { paramsStart: number; paramsEnd: number } | undefined {
  const chars = visibleChars(line);
  const parenStart = chars.findIndex((char) => char.char === '(');
  if (parenStart < 0) return;

  const beforeParen = chars
    .slice(0, parenStart)
    .map((char) => char.char)
    .join('')
    .trimEnd();
  if (beforeParen && !beforeParen.endsWith(':')) return;

  let parenDepth = 0;
  let braceDepth = 0;
  let bracketDepth = 0;
  let angleDepth = 0;

  for (let i = parenStart; i < chars.length; i++) {
    const char = chars[i].char;
    if (char === '(') parenDepth++;
    else if (char === ')') {
      parenDepth--;
      if (parenDepth === 0) {
        const afterParen = chars
          .slice(i + 1)
          .map((entry) => entry.char)
          .join('')
          .trimStart();
        if (afterParen.startsWith(':') || afterParen.startsWith('=>')) {
          return { paramsStart: chars[parenStart].index, paramsEnd: chars[i].index };
        }
        return undefined;
      }
    } else if (char === '{') braceDepth++;
    else if (char === '}') braceDepth = Math.max(0, braceDepth - 1);
    else if (char === '[') bracketDepth++;
    else if (char === ']') bracketDepth = Math.max(0, bracketDepth - 1);
    else if (char === '<' && parenDepth > 0 && braceDepth === 0 && bracketDepth === 0) angleDepth++;
    else if (char === '>' && angleDepth > 0) angleDepth--;
  }

  return undefined;
}

function splitTopLevelParams(text: string): string[] {
  const chars = visibleChars(text);
  const result: string[] = [];
  let start = 0;
  let braceDepth = 0;
  let bracketDepth = 0;
  let parenDepth = 0;
  let angleDepth = 0;
  let stringState: StringLiteralState = {};

  for (const { char, index } of chars) {
    stringState = updateStringLiteralState(stringState, char);
    if (stringState.active || stringState.justEnteredOrExited) continue;

    if (char === '{') braceDepth++;
    else if (char === '}') braceDepth = Math.max(0, braceDepth - 1);
    else if (char === '[') bracketDepth++;
    else if (char === ']') bracketDepth = Math.max(0, bracketDepth - 1);
    else if (char === '(') parenDepth++;
    else if (char === ')') parenDepth = Math.max(0, parenDepth - 1);
    else if (char === '<' && braceDepth === 0 && bracketDepth === 0 && parenDepth === 0)
      angleDepth++;
    else if (char === '>' && angleDepth > 0) angleDepth--;
    else if (
      char === ',' &&
      braceDepth === 0 &&
      bracketDepth === 0 &&
      parenDepth === 0 &&
      angleDepth === 0
    ) {
      result.push(text.slice(start, index));
      start = index + 1;
    }
  }

  result.push(text.slice(start));
  return result.map(trimVisible).filter(Boolean);
}

function wrapParamLine(line: string, maxLineLength: number): string[] {
  if (visibleLength(line) <= maxLineLength) return [line];

  const colonIndex = findTopLevelChar(line, ':');
  if (colonIndex === undefined) return [line];

  const trailingComma = trimVisibleEnd(line).endsWith(',') ? ',' : '';
  const withoutComma = trailingComma ? trimVisibleEnd(line).slice(0, -1) : line;
  const unionParts = splitTopLevelUnion(withoutComma.slice(colonIndex + 1));
  if (unionParts.length <= 1) return [line];

  const firstPrefix = trimVisibleEnd(withoutComma.slice(0, colonIndex + 1));
  const continuationIndent = `${line.match(/^\s*/)?.[0] ?? ''}  `;
  return unionParts.map((part, index) => {
    const rendered = index === 0 ? `${firstPrefix} ${part}` : `${continuationIndent}| ${part}`;
    return index === unionParts.length - 1 ? `${rendered}${trailingComma}` : rendered;
  });
}

function splitTopLevelUnion(text: string): string[] {
  const chars = visibleChars(text);
  const result: string[] = [];
  let start = 0;
  let braceDepth = 0;
  let bracketDepth = 0;
  let parenDepth = 0;
  let angleDepth = 0;
  let stringState: StringLiteralState = {};

  for (const { char, index } of chars) {
    stringState = updateStringLiteralState(stringState, char);
    if (stringState.active || stringState.justEnteredOrExited) continue;

    if (char === '{') braceDepth++;
    else if (char === '}') braceDepth = Math.max(0, braceDepth - 1);
    else if (char === '[') bracketDepth++;
    else if (char === ']') bracketDepth = Math.max(0, bracketDepth - 1);
    else if (char === '(') parenDepth++;
    else if (char === ')') parenDepth = Math.max(0, parenDepth - 1);
    else if (char === '<' && braceDepth === 0 && bracketDepth === 0 && parenDepth === 0)
      angleDepth++;
    else if (char === '>' && angleDepth > 0) angleDepth--;
    else if (
      char === '|' &&
      braceDepth === 0 &&
      bracketDepth === 0 &&
      parenDepth === 0 &&
      angleDepth === 0
    ) {
      result.push(text.slice(start, index));
      start = index + 1;
    }
  }

  result.push(text.slice(start));
  return result.map(trimVisible).filter(Boolean);
}

function findTopLevelChar(text: string, needle: string): number | undefined {
  let stringState: StringLiteralState = {};
  for (const { char, index } of visibleChars(text)) {
    stringState = updateStringLiteralState(stringState, char);
    if (stringState.active || stringState.justEnteredOrExited) continue;

    if (char === needle) return index;
  }
  return undefined;
}

type StringLiteralState = {
  quote?: '"' | "'" | '`';
  escaped?: boolean;
  active?: boolean;
  justEnteredOrExited?: boolean;
};

function updateStringLiteralState(state: StringLiteralState, char: string): StringLiteralState {
  if (state.quote) {
    if (state.escaped) {
      return { ...state, escaped: false, active: true, justEnteredOrExited: false };
    }

    if (char === '\\') {
      return { ...state, escaped: true, active: true, justEnteredOrExited: false };
    }

    if (char === state.quote) {
      return { justEnteredOrExited: true };
    }

    return { ...state, active: true, justEnteredOrExited: false };
  }

  if (char === '"' || char === "'" || char === '`') {
    return { quote: char, active: true, justEnteredOrExited: true };
  }

  return {};
}

function trimVisible(value: string): string {
  return trimVisibleEnd(trimVisibleStart(value));
}

function trimVisibleStart(value: string): string {
  const chars = visibleChars(value);
  const first = chars.find((entry) => !/\s/.test(entry.char));
  return first ? value.slice(first.index) : '';
}

function trimVisibleEnd(value: string): string {
  const chars = visibleChars(value);
  const last = [...chars].reverse().find((entry) => !/\s/.test(entry.char));
  return last ? value.slice(0, last.index + 1) : '';
}

function visibleChars(text: string): Array<{ char: string; index: number }> {
  const result: Array<{ char: string; index: number }> = [];
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '\u001b') {
      const match = /^\u001b\[[0-9;]*m/.exec(text.slice(i));
      if (match) {
        i += match[0].length - 1;
        continue;
      }
    }
    result.push({ char: text[i], index: i });
  }
  return result;
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
  const golemError = asGolemServiceError(error);
  if (golemError) {
    return formatGolemServiceError(golemError);
  }

  if (error instanceof Error) {
    return error.stack ?? error.message;
  }
  return String(error);
}

type GolemServiceErrorLike = {
  message: string;
  stack?: string;
  status?: number;
  statusText?: string;
  code?: string;
  messages?: string[];
  responseFallback?: string[];
  agentError?: GolemAgentErrorDetails;
};

function asGolemServiceError(error: unknown): GolemServiceErrorLike | undefined {
  if (error instanceof GolemServiceError) {
    return {
      message: error.message,
      stack: error.stack,
      status: error.status,
      statusText: error.statusText,
      ...getServiceResponseFields(error),
      agentError: getAgentError(error),
    };
  }
  if (!isRecord(error)) return undefined;
  if (error.name !== 'GolemServiceError') return undefined;
  if (typeof error.message !== 'string') return undefined;

  return {
    message: error.message,
    stack: typeof error.stack === 'string' ? error.stack : undefined,
    agentError: error instanceof GolemServiceError ? getAgentError(error) : undefined,
  };
}

function formatGolemServiceError(error: GolemServiceErrorLike): string {
  const lines = formatServiceResponse(error);

  if (error.agentError?.cause.trim()) {
    const trap = extractWasmTrap(error.agentError.cause);
    if (trap) {
      lines.push('');
      lines.push(`${pc.dim('Wasm trap:')} ${pc.dim(trap)}`);
    }

    const stderr = trimEmptyLines(error.agentError.stderr.split('\n'));
    if (stderr.length > 0) {
      lines.push('');
      lines.push(pc.dim('Stderr:'));
      lines.push(...stderr.map(colorizeGolemServiceErrorLine));
    }

    lines.push('');
    lines.push(pc.dim('Cause:'));
    lines.push(...formatAgentCause(error.agentError.cause));
  }

  const bridgeStack = formatBridgeStack(error.stack, error.message);
  if (bridgeStack.length > 0) {
    lines.push('');
    lines.push(pc.dim('Bridge stack:'));
    lines.push(...bridgeStack);
  }

  return lines.join('\n');
}

function extractWasmTrap(cause: string): string | undefined {
  const trapLine = trimEmptyLines(cause.split('\n'))
    .reverse()
    .find((line) => line.includes('wasm trap:'));
  return trapLine?.split('wasm trap:').pop()?.trim();
}

function formatServiceResponse(error: GolemServiceErrorLike): string[] {
  if (error.status !== undefined) {
    const lines = [pc.dim('Service response:')];
    lines.push(
      `  ${pc.dim('Status:')} ${pc.red(`${error.status} ${error.statusText ?? ''}`.trim())}`,
    );
    if (error.code) {
      lines.push(`  ${pc.dim('Code:')} ${pc.yellow(error.code)}`);
    }
    if (error.messages && error.messages.length === 1) {
      lines.push(`  ${pc.dim('Message:')} ${pc.yellow(error.messages[0])}`);
    } else if (error.messages && error.messages.length > 1) {
      lines.push(`  ${pc.dim('Messages:')}`);
      lines.push(...error.messages.map((message) => `    ${pc.yellow(`- ${message}`)}`));
    }
    if (error.responseFallback) {
      lines.push(...error.responseFallback.map(colorizeGolemServiceErrorLine));
    }
    return lines;
  }

  return error.message.split('\n').map((line) => colorizeGolemServiceErrorLine(line));
}

function getAgentError(error: GolemServiceError): GolemAgentErrorDetails | undefined {
  if (!error.body || !('agentError' in error.body)) return undefined;
  return error.body.agentError;
}

function getServiceResponseFields(error: GolemServiceError): {
  code?: string;
  messages?: string[];
  responseFallback?: string[];
} {
  if (error.body) {
    return 'errors' in error.body
      ? { code: error.body.code, messages: error.body.errors }
      : { code: error.body.code, messages: [error.body.error] };
  }

  return { responseFallback: error.message.split('\n').slice(1) };
}

function formatAgentCause(cause: string): string[] {
  return trimEmptyLines(cause.split('\n')).map((line) => pc.dim(line));
}

function colorizeGolemServiceErrorLine(line: string): string {
  const trimmed = line.trimStart();
  if (line.startsWith('Agent creation failed:') || line.startsWith('Agent invocation failed:')) {
    return pc.red(pc.bold(line));
  }
  if (trimmed.startsWith('JavaScript exception:') || trimmed.startsWith('JavaScript error:')) {
    return pc.red(pc.bold(line));
  }
  if (
    line.startsWith('Code:') ||
    line.startsWith('Messages:') ||
    line.startsWith('Message:') ||
    line.startsWith('Stderr:') ||
    line.startsWith('Wasm trap:') ||
    line.startsWith('Response body:') ||
    line.startsWith('Response message:') ||
    line.startsWith('Response messages:')
  ) {
    return pc.dim(line);
  }
  if (trimmed.startsWith('at ') && !line.includes('(user:')) {
    return pc.dim(line);
  }
  if (
    trimmed.startsWith("thread '") ||
    trimmed.startsWith('note:') ||
    trimmed.startsWith('Exception during awaiting call result')
  ) {
    return pc.dim(line);
  }
  return line;
}

function formatBridgeStack(stack: string | undefined, message: string): string[] {
  if (!stack) return [];

  const lines = stack.split('\n');
  const firstFrameIndex = lines
    .slice(message.split('\n').length)
    .findIndex((line) => line.trimStart().startsWith('at '));
  if (firstFrameIndex < 0) return [];

  const actualFirstFrameIndex = firstFrameIndex + message.split('\n').length;
  return lines
    .slice(actualFirstFrameIndex)
    .filter((line) => line.trim().length > 0 && line !== message)
    .map((line) => pc.dim(line));
}

function trimEmptyLines(lines: string[]): string[] {
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start].trim() === '') start += 1;
  while (end > start && lines[end - 1].trim() === '') end -= 1;
  return lines.slice(start, end);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function writeFullLineSeparator() {
  const width = getTerminalWidth();
  if (!width || width <= 0) return;
  writeln(pc.dim('~'.repeat(width)));
}
