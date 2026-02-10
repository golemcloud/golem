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

import {
  cliCommandsConfigFromBaseConfig,
  ClientConfig,
  clientConfigFromEnv,
  Config,
  ConfigureClient,
  loadProcessArgs,
  ProcessArgs,
} from './config';
import { CliReplInterop } from './cli-repl-interop';
import { LanguageService } from './language-service';
import pc from 'picocolors';
import repl, { type REPLEval } from 'node:repl';
import process from 'node:process';
import util from 'node:util';
import { AsyncCompleter } from 'readline';
import { PassThrough } from 'node:stream';
import { ts } from 'ts-morph';

export class Repl {
  private readonly config: Config;
  private readonly clientConfig: ClientConfig;
  private readonly cli: CliReplInterop;
  private readonly processArgs: ProcessArgs;
  private languageService: LanguageService | undefined;

  constructor(config: Config) {
    this.config = config;
    this.processArgs = loadProcessArgs();
    this.clientConfig = clientConfigFromEnv();
    this.cli = new CliReplInterop(cliCommandsConfigFromBaseConfig(this.config, this.clientConfig));
  }

  private getLanguageService(): LanguageService {
    if (!this.languageService) {
      this.languageService = new LanguageService(this.config, this.processArgs);
    }
    return this.languageService;
  }

  private newBaseReplServer(options?: {
    input?: NodeJS.ReadableStream;
    output?: NodeJS.WritableStream;
    terminal?: boolean;
  }): repl.REPLServer {
    const output = options?.output ?? process.stdout;
    const terminal = options?.terminal ?? Boolean((output as any).isTTY);
    return repl.start({
      input: options?.input ?? process.stdin,
      output,
      terminal,
      useColors: pc.isColorSupported,
      useGlobal: true,
      preview: false,
      ignoreUndefined: true,
      prompt:
        `${pc.cyan('golem-ts-repl')}` +
        `[${pc.bold(pc.green(this.clientConfig.application))}]` +
        `[${pc.bold(pc.yellow(this.clientConfig.environment))}]` +
        `${pc.bold(pc.green('>'))} `,
    });
  }

  private async setupRepl(replServer: repl.REPLServer): Promise<repl.REPLEval> {
    await this.setupReplHistory(replServer);
    const tsxEval = this.setupReplEval(replServer);
    this.setupReplCompleter(replServer);
    this.setupReplContext(replServer);
    this.setupReplCommands(replServer);
    return tsxEval;
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

  private setupReplEval(replServer: repl.REPLServer): repl.REPLEval {
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

        replMessageSink.writeText(typeCheckResult.formattedErrors);

        callback(null, undefined);
      }
    };
    (replServer.eval as any) = customEval;

    return tsxEval;
  }

  private setupReplCompleter(replServer: repl.REPLServer) {
    const nodeCompleter = replServer.completer;
    const languageService = this.getLanguageService();
    const cli = this.cli;
    const customCompleter: AsyncCompleter = function (line, callback) {
      // TODO: how to check for builtin ones
      if (line.trimStart().startsWith('.')) {
        cli
          .complete(line)
          .then((result) => {
            if (result) {
              callback(null, result);
            } else {
              nodeCompleter(line, callback);
            }
          })
          .catch(() => {
            nodeCompleter(line, callback);
          });
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
    if (this.processArgs.disableAutoImports) {
      return;
    }

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
    this.cli.defineCommands(replServer);
    replServer.defineCommand('reload', {
      help: 'Reload the REPL',
      action() {
        CliReplInterop.exitWithReloadCode();
      },
    });
  }

  async run() {
    const script = this.processArgs.script;
    const replServer = script
      ? this.newBaseReplServer({
          input: new PassThrough(),
          output: new PassThrough(),
          terminal: false,
        })
      : this.newBaseReplServer();

    const tsxEval = await this.setupRepl(replServer);

    if (script) {
      await this.runScript(tsxEval, replServer, script);
      replServer.close();
    }
  }

  private async runScript(tsxEval: repl.REPLEval, replServer: repl.REPLServer, script: string) {
    const previousSink = replMessageSink;
    replMessageSink = stderrMessageSink;

    let evalResult: { error: Error | null; result: unknown };
    try {
      const preparedScript = prepareScriptForEval(script);
      const filename = this.processArgs.scriptPath ?? 'repl-script';
      const evalFn = preparedScript.transpiled ? tsxEval : replServer.eval;
      evalResult = await new Promise((resolve) => {
        evalFn.call(
          replServer,
          preparedScript.script,
          replServer.context,
          filename,
          (err, result) => {
            resolve({ error: err as Error | null, result });
          },
        );
      });
    } finally {
      replMessageSink = previousSink;
    }

    if (evalResult.error) {
      process.stderr.write(formatEvalError(evalResult.error));
      return;
    }

    const jsonResult = tryJsonStringify(evalResult.result);
    if (jsonResult !== undefined) {
      process.stdout.write(jsonResult);
      return;
    }

    this.printReplResult(replServer, evalResult.result);
  }

  private printReplResult(replServer: repl.REPLServer, result: unknown) {
    if (result === undefined && replServer.ignoreUndefined) return;
    const rendered = replServer.writer(result);
    process.stdout.write(rendered);
  }
}

const INFO_PREFIX = pc.bold(pc.red('>'));
const INFO_PREFIX_LENGTH = util.stripVTControlCharacters(INFO_PREFIX).length + 1;

type ReplMessageSink = {
  writeText: (text: string) => void;
};

let replMessageSink: ReplMessageSink = {
  writeText: (text: string) => {
    console.log(text);
  },
};

const stderrMessageSink: ReplMessageSink = {
  writeText: (text: string) => {
    process.stderr.write(text + '\n');
  },
};

function logSnippetInfo(message: string) {
  if (!message) return;

  let maxLineLength = 0;
  message.split('\n').forEach((line) => {
    maxLineLength = Math.max(maxLineLength, util.stripVTControlCharacters(line).length);
    replMessageSink.writeText(`${INFO_PREFIX} ${line}`);
  });

  if (maxLineLength > 0) {
    replMessageSink.writeText(
      pc.dim('~'.repeat(Math.min(maxLineLength + INFO_PREFIX_LENGTH, terminalWidth))),
    );
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

function formatEvalError(error: unknown) {
  if (error instanceof Error) {
    return error.stack ?? error.message;
  }
  return String(error);
}

function tryJsonStringify(value: unknown): string | undefined {
  try {
    const json = JSON.stringify(value);
    return json === undefined ? undefined : json;
  } catch {
    return undefined;
  }
}

function prepareScriptForEval(script: string): { script: string; transpiled: boolean } {
  if (!hasModuleSyntax(script)) {
    return { script, transpiled: false };
  }

  const output = ts.transpileModule(script, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });

  return { script: output.outputText, transpiled: true };
}

function hasModuleSyntax(script: string): boolean {
  const sourceFile = ts.createSourceFile(
    'repl-script.ts',
    script,
    ts.ScriptTarget.ES2020,
    true,
    ts.ScriptKind.TS,
  );

  let hasModule = false;
  const visit = (node: ts.Node) => {
    if (
      ts.isImportDeclaration(node) ||
      ts.isExportDeclaration(node) ||
      ts.isExportAssignment(node) ||
      ts.isImportEqualsDeclaration(node)
    ) {
      hasModule = true;
      return;
    }
    ts.forEachChild(node, visit);
  };

  ts.forEachChild(sourceFile, visit);
  return hasModule;
}
