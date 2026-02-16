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

import process from 'node:process';
import { Writable } from 'node:stream';

type OutputKind = 'stdout' | 'stderr';

type Output = {
  writeln: (text: string) => void;
  writeChunk: (chunk: Buffer) => void;
};

const stdoutOutput: Output = {
  writeln(text: string) {
    process.stdout.write(text + '\n');
  },
  writeChunk(chunk: Buffer) {
    process.stdout.write(chunk);
  },
};

const stderrOutput: Output = {
  writeln(text: string) {
    process.stderr.write(text + '\n');
  },
  writeChunk(chunk: Buffer) {
    process.stderr.write(chunk);
  },
};

let currentOutput: Output = stdoutOutput;

export function getOutput(): Output {
  return currentOutput;
}

export function setOutput(sinkKind: OutputKind): void {
  switch (sinkKind) {
    case 'stdout':
      currentOutput = stdoutOutput;
      break;
    case 'stderr':
      currentOutput = stderrOutput;
      break;
    default:
      assertNever(sinkKind);
  }
}

export function writeln(text: string): void {
  currentOutput.writeln(text);
}

export function writeChunk(chunk: Buffer): void {
  currentOutput.writeChunk(chunk);
}

export function flushStream(stream: Writable): Promise<void> {
  return new Promise((resolve) => {
    if (!stream.writableNeedDrain) {
      resolve();
      return;
    }
    stream.once('drain', resolve);
  });
}

export async function flushStdIO(): Promise<void> {
  await Promise.all([flushStream(process.stdout), flushStream(process.stderr)]);
}

let terminalWidth = process.stdout.isTTY ? process.stdout.columns : 80;

if (process.stdout.isTTY) {
  process.stdout.on('resize', () => {
    terminalWidth = process.stdout.columns;
  });
}

export function getTerminalWidth(): number {
  return terminalWidth;
}

function assertNever(x: never): never {
  throw new Error('Unexpected object: ' + x);
}
