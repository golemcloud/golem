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

import { client_configuration_from_env, Config, ConfigureClient } from './config';
import { LanguageService } from './language-service';
import pc from 'picocolors';
import repl, { type REPLEval } from 'node:repl';
import process from 'node:process';
import shellQuote from 'shell-quote';
import childProcess from 'node:child_process';
import util from 'node:util';

export class Repl {
  private readonly config: Config;

  constructor(config: Config) {
    this.config = config;
  }

  async run() {
    const clientConfig = client_configuration_from_env();
    let languageService = new LanguageService(this.config);

    const r = repl.start({
      useColors: pc.isColorSupported,
      useGlobal: true,
      preview: false,
      ignoreUndefined: true,
      prompt:
        `${pc.cyan('golem-ts-repl')}` +
        `[${pc.green(clientConfig.application)}]` +
        `[${pc.yellow(clientConfig.environment)}]` +
        `${pc.red('>')} `,
    });

    await new Promise<void>((resolve, reject) => {
      r.setupHistory(this.config.historyFile, (err) => {
        if (err) {
          reject(err);
        } else {
          resolve();
        }
      });
    });

    {
      const tsxEval = r.eval;
      const customEval: REPLEval = function (code, context, filename, callback) {
        const evalCode = (code: string) => {
          tsxEval.call(this, code, context, filename, (err, result) => {
            if (!err) {
              languageService.addSnippetToHistory(code);
            }
            callback(err, result);
          });
        };

        languageService.setSnippet(code);
        const typeCheckResult = languageService.typeCheckSnippet();
        if (typeCheckResult.ok) {
          const quickInfo = languageService.getSnippetQuickInfo();
          const typeInfo = languageService.getSnippetTypeInfo();

          if (typeInfo && typeInfo.isPromise) {
            logSnippetInfo(pc.bold('awaiting ' + typeInfo.formattedType));
            evalCode('await ' + code);
          } else {
            if (quickInfo) {
              logSnippetInfo(pc.bold(quickInfo.formattedInfo));
            } else if (typeInfo) {
              logSnippetInfo(pc.bold(typeInfo.formattedType));
            }
            evalCode(code);
          }
        } else {
          const completions = languageService.getSnippetCompletions();
          if (completions && completions.entries.length) {
            let entries = completions.entries;
            if (completions.entries.length > MAX_COMPLETION_ENTRIES) {
              entries = completions.entries.slice(0, MAX_COMPLETION_ENTRIES - 1);
              entries.push('...');
            }

            logSnippetInfo(
              pc.bold('Completions:') +
                `\n` +
                formatAsTable(entries, {
                  maxLineLength: terminalWidth - INFO_PREFIX_LENGTH,
                }),
            );
          }

          console.log(typeCheckResult.formattedErrors);

          callback(null, undefined);
        }
      };
      (r.eval as any) = customEval;
    }

    for (let agentTypeName in this.config.agents) {
      const agentConfig = this.config.agents[agentTypeName];
      let configure = agentConfig.package.configure as ConfigureClient;
      configure(clientConfig);
      r.context[agentTypeName] = agentConfig.package[agentTypeName];
      r.context[agentConfig.clientPackageImportedName] = agentConfig.package;
    }

    r.defineCommand('deploy', {
      help: 'Deploy the current Golem Application',
      async action(raw_args: string) {
        this.pause();

        const parsed_args = shellQuote.parse((raw_args ?? '').trim());

        let args = parsed_args.filter((t): t is string => typeof t === 'string' && t.length > 0);
        args = [
          'deploy',
          '--environment',
          clientConfig.environment,
          '--repl-bridge-sdk-target',
          'ts',
          ...args,
        ];

        const child = childProcess.spawn('golem', args, { stdio: 'inherit' });
        let result: {
          code: number | null;
          signal: NodeJS.Signals | null;
        } = await new Promise((resolve) =>
          child.once('exit', (code: number | null, signal: NodeJS.Signals | null) => {
            resolve({ code, signal });
          }),
        );

        if (result.code === 0) {
          reload();
        }

        this.resume();
        this.displayPrompt();
        this.clearBufferedCommand();
      },
    });

    r.defineCommand('reload', {
      help: 'Reload the REPL',
      action() {
        reload();
      },
    });
  }
}

function reload() {
  process.exit(75);
}

const INFO_PREFIX = pc.red('>');
const INFO_PREFIX_LENGTH = util.stripVTControlCharacters(INFO_PREFIX).length + 1;

function logSnippetInfo(message: string) {
  if (!message) {
    return;
  }

  let maxLineLength = 0;
  message.split('\n').forEach((line) => {
    maxLineLength = Math.max(maxLineLength, util.stripVTControlCharacters(line).length);
    console.log(INFO_PREFIX, line);
  });

  if (maxLineLength > 0) {
    console.log(pc.dim('~'.repeat(Math.min(maxLineLength + INFO_PREFIX_LENGTH, terminalWidth))));
  }
}

type FormatAsTableOptions = {
  maxLineLength: number;
  separator?: string;
};

export function formatAsTable(
  items: string[],
  { maxLineLength, separator = '  ' }: FormatAsTableOptions,
): string {
  if (items.length === 0) return '';

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

const MAX_COMPLETION_ENTRIES = 20;

let terminalWidth = process.stdout.isTTY ? process.stdout.columns : 80;

if (process.stdout.isTTY) {
  process.stdout.on('resize', () => {
    terminalWidth = process.stdout.columns;
  });
}
