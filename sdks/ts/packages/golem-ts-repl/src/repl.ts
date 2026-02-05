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

import { ClientConfiguration, clientConfigurationFromEnv, Config, ConfigureClient } from './config';
import { LanguageService } from './language-service';
import pc from 'picocolors';
import repl, { type REPLEval } from 'node:repl';
import process from 'node:process';
import shellQuote from 'shell-quote';
import childProcess from 'node:child_process';
import util from 'node:util';
import { AsyncCompleter } from 'readline';

export class Repl {
  private readonly config: Config;
  private readonly clientConfig: ClientConfiguration;
  private languageService: LanguageService | undefined;
  private replServer: repl.REPLServer | undefined;

  constructor(config: Config) {
    this.config = config;
    this.clientConfig = clientConfigurationFromEnv();
  }

  private getLanguageService(): LanguageService {
    if (!this.languageService) {
      this.languageService = new LanguageService(this.config);
    }
    return this.languageService;
  }

  private async getReplServer(): Promise<repl.REPLServer> {
    if (!this.replServer) {
      const replServer = this.newBaseReplServer();
      await this.setupRepl(replServer);
      this.replServer = replServer;
    }
    return this.replServer!;
  }

  private newBaseReplServer(): repl.REPLServer {
    return repl.start({
      useColors: pc.isColorSupported,
      useGlobal: true,
      preview: false,
      ignoreUndefined: true,
      prompt:
        `${pc.cyan('golem-ts-repl')}` +
        `[${pc.green(this.clientConfig.application)}]` +
        `[${pc.yellow(this.clientConfig.environment)}]` +
        `${pc.red('>')} `,
    });
  }

  private async setupRepl(replServer: repl.REPLServer) {
    await this.setupReplHistory(replServer);
    this.setupReplEval(replServer);
    this.setupReplCompleter(replServer);
    this.setupReplContext(replServer);
    this.setupReplCommands(replServer);
  }

  private async setupReplHistory(replServer: repl.REPLServer) {
    await new Promise<void>((resolve, reject) => {
      replServer.setupHistory(this.config.historyFile, (err) => {
        if (err) {
          reject(err);
        } else {
          resolve();
        }
      });
    });
  }

  private setupReplEval(replServer: repl.REPLServer) {
    const tsxEval = replServer.eval;
    const languageService = this.getLanguageService();

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
            formatAsTable(entries, {
              maxLineLength: terminalWidth - INFO_PREFIX_LENGTH,
            }),
          );
        }

        console.log(typeCheckResult.formattedErrors);

        callback(null, undefined);
      }
    };
    (replServer.eval as any) = customEval;
  }

  private setupReplCompleter(replServer: repl.REPLServer) {
    const nodeCompleter = replServer.completer;
    const languageService = this.getLanguageService();
    const customCompleter: AsyncCompleter = function (line, callback) {
      if (line.trimStart().startsWith('.')) {
        nodeCompleter(line, callback);
      } else {
        languageService.setSnippet(line);
        const completions = languageService.getSnippetCompletions();
        if (completions && completions.entries.length) {
          const replaceStart = completions.replaceStart ?? 0;
          const replaceEnd = completions.replaceEnd ?? line.length;
          const completeOn = line.slice(replaceStart, replaceEnd);
          callback(null, [completions.entries, completeOn]);
        } else {
          callback(null, [[], '']);
        }
      }
    };
    (replServer.completer as any) = customCompleter;
  }

  private setupReplContext(replServer: repl.REPLServer) {
    const context = replServer.context;
    for (let agentTypeName in this.config.agents) {
      const agentConfig = this.config.agents[agentTypeName];
      let configure = agentConfig.package.configure as ConfigureClient;
      configure(this.clientConfig);
      context[agentTypeName] = agentConfig.package[agentTypeName];
      context[agentConfig.clientPackageImportedName] = agentConfig.package;
    }
  }

  private setupReplCommands(replServer: repl.REPLServer) {
    const clientConfig = this.clientConfig;
    replServer.defineCommand('deploy', {
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

    replServer.defineCommand('reload', {
      help: 'Reload the REPL',
      action() {
        reload();
      },
    });
  }

  async run() {
    await this.getReplServer();
  }
}

function reload() {
  process.exit(75);
}

const INFO_PREFIX = pc.red('>');
const INFO_PREFIX_LENGTH = util.stripVTControlCharacters(INFO_PREFIX).length + 1;

function logSnippetInfo(message: string) {
  if (!message) return;

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
