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

import {
  cliCommandsConfigFromBaseConfig,
  clientConfigFromEnv,
  Config,
  ConfigureClient,
  loadReplCliFlags,
  ReplCliFlags,
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
import { flushStdIO, setOutput, writeln } from './process';
import { initTestSyncEventsFromEnv, writeTestSyncEvent } from './test-sync-events';
import { formatAsTable, formatEvalError, logSnippetInfo } from './format';
import type * as base from '@golemcloud/golem-ts-bridge';
import type {
  AgentInvocationRequest,
  AgentInvocationResult,
  JsonResult,
} from '@golemcloud/golem-ts-bridge';

const MAX_COMPLETION_ENTRIES = 50;

export class Repl {
  private readonly config: Config;
  private readonly clientConfig: base.Configuration;
  private readonly cli: CliReplInterop;
  private readonly replCliFlags: ReplCliFlags;
  private languageService: LanguageService | undefined;
  private overrideSnippetForNextEval: string | undefined;

  constructor(config: Config) {
    this.config = config;
    initTestSyncEventsFromEnv();
    this.replCliFlags = loadReplCliFlags();
    this.overrideSnippetForNextEval = undefined;

    const clientConfig = clientConfigFromEnv();
    this.clientConfig = clientConfig;

    this.cli = new CliReplInterop(cliCommandsConfigFromBaseConfig(this.config, this.clientConfig));
    const cli = this.cli;
    const replCliFlags = this.replCliFlags;
    clientConfig.aroundInvokeHook = {
      async beforeInvoke(request: AgentInvocationRequest): Promise<void> {
        if (replCliFlags.streamLogs) {
          cli.startAgentStream(request);
        }
      },

      async afterInvoke(
        request: AgentInvocationRequest,
        result: JsonResult<AgentInvocationResult, any>,
      ): Promise<void> {
        void result;
        await cli.stopAgentStream(request);
      },
    };
  }

  private getLanguageService(): LanguageService {
    if (!this.languageService) {
      this.languageService = new LanguageService(this.config, this.replCliFlags);
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
    const scriptMode = Boolean(this.replCliFlags.script);
    const prompt = this.replCliFlags.script
      ? ''
      : `${pc.cyan('golem-ts-repl')}` +
        `[${pc.bold(pc.green(this.clientConfig.application))}]` +
        `[${pc.bold(pc.yellow(this.clientConfig.environment))}]` +
        `${pc.bold(pc.green('>'))} `;

    return repl.start({
      input: options?.input ?? process.stdin,
      output,
      terminal,
      ...(scriptMode ? { eval: createEvalWithErrors() } : {}),
      useColors: pc.isColorSupported,
      useGlobal: true,
      preview: false,
      ignoreUndefined: true,
      prompt,
      breakEvalOnSigint: !this.replCliFlags.script,
      writer: (value) => util.inspect(value, { depth: null, colors: pc.isColorSupported }),
    });
  }

  private async setupRepl(replServer: repl.REPLServer): Promise<void> {
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

  private setupReplEval(replServer: repl.REPLServer): repl.REPLEval {
    const tsxEval = replServer.eval;
    const languageService = this.getLanguageService();
    const replCliFlags = this.replCliFlags;
    const getOverrideSnippet = () => this.overrideSnippetForNextEval;

    const customEval: REPLEval = function (code, context, filename, callback) {
      if (tryHandleColonCommand(code, replServer, callback)) {
        return;
      }

      const evalCode = (code: string) => {
        tsxEval.call(this, code, context, filename, (err, result) => {
          if (!err) {
            languageService.addSnippetToHistory(code);
          }
          writeTestSyncEvent('eval_done');
          callback(err, result);
        });
      };

      const failScript = (message: string) => {
        const error = new Error(message);
        writeTestSyncEvent('eval_done');
        callback(error, undefined);
      };

      const snippet = getOverrideSnippet() ?? code;
      languageService.setSnippet(snippet);
      const typeCheckResult = languageService.typeCheckSnippet();
      if (typeCheckResult.ok) {
        const quickInfo = languageService.getSnippetQuickInfo();
        const typeInfo = languageService.getSnippetTypeInfo();

        if (typeInfo && typeInfo.isPromise) {
          if (replCliFlags.showTypeInfo) {
            logSnippetInfo(pc.bold('awaiting ' + typeInfo.formattedType));
          }
          // Script mode uses a custom base eval that awaits returned promises; interactive mode
          // keeps Node's REPL top-level-await behavior for promise expressions.
          evalCode(replCliFlags.script ? code : 'await ' + code);
        } else {
          if (replCliFlags.showTypeInfo) {
            if (quickInfo) {
              logSnippetInfo(pc.bold(quickInfo.formattedInfo));
            } else if (typeInfo) {
              logSnippetInfo(pc.bold(typeInfo.formattedType));
            }
          }
          evalCode(code);
        }
      } else {
        const formattedHints = typeCheckResult.formattedHints;
        const completions = languageService.getSnippetCompletions({
          includePlaceholders: formattedHints.length === 0,
        });
        if (completions && completions.entries.length) {
          let entries = completions.entries;
          if (completions.entries.length > MAX_COMPLETION_ENTRIES) {
            entries = completions.entries.slice(0, MAX_COMPLETION_ENTRIES - 1);
            entries.push('...');
          }

          logSnippetInfo(formatAsTable(entries));
        }

        if (formattedHints.length) {
          logSnippetInfo(formattedHints);
        }

        writeln(typeCheckResult.formattedErrors);

        if (replCliFlags.script) {
          failScript(typeCheckResult.formattedErrors);
          return;
        }

        writeTestSyncEvent('eval_done');
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
      const callbackWithSync = (err?: Error | null, result?: [string[], string]): void => {
        writeTestSyncEvent('completion_done');
        callback(err, result);
      };

      if (line.trimStart().startsWith('.') || line.trimStart().startsWith(':')) {
        cli
          .complete(line)
          .then((result) => {
            if (result) {
              callbackWithSync(null, result);
            } else {
              nodeCompleter(line, callbackWithSync);
            }
          })
          .catch(() => {
            nodeCompleter(line, callbackWithSync);
          });
      } else {
        languageService.setSnippet(line);
        const completions = languageService.getSnippetCompletions();
        if (completions && completions.entries.length) {
          completions.entries = completions.entries.slice(0, MAX_COMPLETION_ENTRIES);
          const replaceStart = completions.replaceStart ?? 0;
          const replaceEnd = completions.replaceEnd ?? line.length;
          const completeOn = line.slice(replaceStart, replaceEnd);
          callbackWithSync(null, [completions.entries, completeOn]);
        } else {
          callbackWithSync(null, [[], '']);
        }
      }
    };
    (replServer.completer as any) = customCompleter;
  }

  private setupReplContext(replServer: repl.REPLServer) {
    if (this.replCliFlags.disableAutoImports) {
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
    replServer.defineCommand('quit', {
      help: 'Quit the REPL (exit alias)',
      action: () => {
        replServer.close();
      },
    });

    replServer.defineCommand('reload', {
      help: 'Reload the REPL',
      action() {
        void CliReplInterop.exitWithReloadCode();
      },
    });

    replServer.defineCommand('agent-type-info', {
      help: 'Show auto-imported agent client info',
      action: () => {
        this.showAgentTypeInfo(replServer, true);
      },
    });

    this.defineFlagCommand(replServer, {
      name: 'stream-logs',
      help: 'Show or set agent stream logging (on/off)',
      get: () => this.replCliFlags.streamLogs,
      set: (value) => {
        this.replCliFlags.streamLogs = value;
      },
    });

    this.defineFlagCommand(replServer, {
      name: 'show-type-info',
      help: 'Show or set type info logging before execution (on/off)',
      get: () => this.replCliFlags.showTypeInfo,
      set: (value) => {
        this.replCliFlags.showTypeInfo = value;
      },
    });

    this.cli.defineCommands(replServer);
  }

  private defineFlagCommand(
    replServer: repl.REPLServer,
    opts: {
      name: string;
      help: string;
      get: () => boolean;
      set: (value: boolean) => void;
    },
  ) {
    replServer.defineCommand(opts.name, {
      help: opts.help,
      action: (rawArgs: string) => {
        const trimmed = rawArgs.trim();
        if (!trimmed) {
          logSnippetInfo(`${opts.name} is ${opts.get() ? pc.green('on') : pc.red('off')}`);
          replServer.displayPrompt();
          return;
        }

        const parsed = parseToggleValue(trimmed);
        if (parsed === undefined) {
          logSnippetInfo(`Usage: .${opts.name} [on|off]`);
          replServer.displayPrompt();
          return;
        }

        opts.set(parsed);
        logSnippetInfo(`${opts.name} set to ${opts.get() ? pc.green('on') : pc.red('off')}`);
        replServer.displayPrompt();
      },
    });
  }

  private showAgentTypeInfo(replServer: repl.REPLServer, manual = false) {
    if (this.replCliFlags.disableAutoImports) return;

    const agentNames = Object.keys(this.config.agents).sort((a, b) => a.localeCompare(b));
    if (agentNames.length === 0) return;

    const languageService = this.getLanguageService();
    const lines: string[] = [];
    lines.push('');
    lines.push(pc.bold('Available agent client types:'));

    for (const agentTypeName of agentNames) {
      const methods = languageService.getClientMethodSignatures(agentTypeName);
      const primaryFactoryMethodSignature =
        languageService.getAgentTypePrimaryFactoryMethodSignature(agentTypeName);
      const header = primaryFactoryMethodSignature ?? agentTypeName;
      lines.push(`  ${pc.bold(header)}`);

      if (methods?.length) {
        for (const method of methods) {
          lines.push(`    ${pc.green(method.name)}: ${method.signature}`);
        }
      }

      lines.push('');
    }

    if (!manual) {
      lines.push(pc.dim('To see this message again, use the `.agent-type-info` command!'));
      replServer.output.write('\n');
    }
    logSnippetInfo(lines);
    replServer.displayPrompt();
  }

  async run() {
    setOutput('stdout');

    const script = this.replCliFlags.script;
    const replServer = script
      ? this.newBaseReplServer({
          input: new PassThrough(),
          output: process.stdout,
          terminal: false,
        })
      : this.newBaseReplServer();

    await this.setupRepl(replServer);

    if (script) {
      await this.runScript(replServer, script);
      await flushStdIO();
      replServer.close();
    } else {
      replServer.once('close', () => {
        void CliReplInterop.exitWithCode(0);
      });
      this.showAgentTypeInfo(replServer, false);
      writeTestSyncEvent('repl_ready');
    }
  }

  private async runScript(replServer: repl.REPLServer, script: string) {
    setOutput('stderr');

    let evalResult: { error: Error | null; result: unknown };
    try {
      const preparedScript = prepareScriptForEval(script);
      const filename = this.replCliFlags.scriptPath ?? 'repl-script';
      this.overrideSnippetForNextEval = script;
      evalResult = await new Promise((resolve) => {
        replServer.eval(preparedScript.script, replServer.context, filename, (err, result) => {
          resolve({ error: err as Error | null, result });
        });
      });
    } finally {
      this.overrideSnippetForNextEval = undefined;
    }

    if (evalResult.error) {
      writeln(formatEvalError(evalResult.error));
      process.exitCode = 1;
      return;
    }

    const finalResult = evalResult.result;

    const jsonResult = tryJsonStringify(finalResult);
    if (jsonResult !== undefined) {
      process.stdout.write(jsonResult + '\n');
      return;
    }

    if (finalResult === undefined && replServer.ignoreUndefined) return;
    const rendered = replServer.writer(finalResult);
    process.stdout.write(rendered + '\n');
  }
}

function createEvalWithErrors(): REPLEval {
  // Node's embedded REPL prints uncaught eval errors but does not report them through the eval
  // callback. Script mode needs callback errors so the outer CLI can return a non-zero status.
  return function (code, _context, _filename, callback) {
    const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor as new (
      body: string,
    ) => () => Promise<unknown>;

    try {
      const result = new AsyncFunction(transformScriptForAsyncEval(code))();
      Promise.resolve(result)
        .then((value) => callback(null, value))
        .catch((error) => callback(normalizeEvalError(error), undefined));
    } catch (error) {
      callback(normalizeEvalError(error), undefined);
    }
  };
}

function transformScriptForAsyncEval(code: string): string {
  const sourceFile = ts.createSourceFile(
    'repl-script.js',
    code,
    ts.ScriptTarget.ES2020,
    true,
    ts.ScriptKind.JS,
  );

  const lastStatement = sourceFile.statements[sourceFile.statements.length - 1];
  if (!lastStatement || !ts.isExpressionStatement(lastStatement)) {
    return code;
  }

  const expressionStart = lastStatement.expression.getStart(sourceFile, false);
  const expressionEnd = lastStatement.expression.getEnd();
  const prefix = code.slice(0, lastStatement.getFullStart());
  const expression = code.slice(expressionStart, expressionEnd);

  return `${prefix}return await (${expression});`;
}

function normalizeEvalError(error: unknown): Error {
  if (error instanceof Error) {
    return error;
  }

  const normalized = new Error(String(error));
  normalized.stack = normalized.message;
  return normalized;
}

function tryHandleColonCommand(
  line: string,
  replServer: repl.REPLServer,
  callback: (err: Error | null, result?: any) => void,
): boolean {
  const trimmed = line.trimStart();
  if (!trimmed.startsWith(':')) {
    return false;
  }
  const commandLine = trimmed.slice(1).trimStart();
  if (!commandLine) {
    callback(null, undefined);
    return true;
  }
  const whitespaceIndex = commandLine.search(/\s/);
  const commandName = whitespaceIndex === -1 ? commandLine : commandLine.slice(0, whitespaceIndex);
  const rawArgs = whitespaceIndex === -1 ? '' : commandLine.slice(whitespaceIndex + 1);
  const command = (replServer as any).commands?.[commandName];
  if (command?.action) {
    try {
      const result = command.action.call(replServer, rawArgs);
      if (result && typeof (result as Promise<void>).then === 'function') {
        Promise.resolve(result)
          .then(() => callback(null, undefined))
          .catch((err) => callback(err as Error, undefined));
        return true;
      }
    } catch (err) {
      callback(err as Error, undefined);
      return true;
    }
    callback(null, undefined);
    return true;
  }
  writeln(`Unknown command: ${commandName}`);
  callback(null, undefined);
  return true;
}

function tryJsonStringify(value: unknown): string | undefined {
  try {
    const json = JSON.stringify(value, null, 2);
    return json === undefined ? undefined : json;
  } catch {
    return undefined;
  }
}

function prepareScriptForEval(script: string): { script: string; transformed: boolean } {
  const sourceFile = ts.createSourceFile(
    'repl-script.ts',
    script,
    ts.ScriptTarget.ES2020,
    true,
    ts.ScriptKind.TS,
  );

  const edits: { start: number; end: number; replacement: string }[] = [];

  for (const statement of sourceFile.statements) {
    if (!ts.isImportDeclaration(statement)) {
      continue;
    }

    const replacement = rewriteImportToDynamic(statement, sourceFile);
    const start = statement.getStart(sourceFile, false);
    const end = statement.getEnd();
    edits.push({ start, end, replacement });
  }

  if (edits.length === 0) {
    return { script, transformed: false };
  }

  let updated = script;
  for (const edit of edits.sort((a, b) => b.start - a.start)) {
    const original = updated.slice(edit.start, edit.end);
    const originalLines = countNewlines(original);
    const replacementLines = countNewlines(edit.replacement);
    let replacement = edit.replacement;
    if (replacementLines < originalLines) {
      replacement += '\n'.repeat(originalLines - replacementLines);
    }
    updated = updated.slice(0, edit.start) + replacement + updated.slice(edit.end);
  }

  return { script: updated, transformed: true };
}

function rewriteImportToDynamic(
  statement: ts.ImportDeclaration,
  sourceFile: ts.SourceFile,
): string {
  const importClause = statement.importClause;
  const moduleText = statement.moduleSpecifier.getText(sourceFile);

  if (!importClause) {
    return `await import(${moduleText});`;
  }

  if (importClause.isTypeOnly) {
    return 'void 0;';
  }

  if (importClause.namedBindings && ts.isNamespaceImport(importClause.namedBindings)) {
    return `const ${importClause.namedBindings.name.text} = await import(${moduleText});`;
  }

  const bindings: string[] = [];
  if (importClause.name) {
    bindings.push(`default: ${importClause.name.text}`);
  }

  if (importClause.namedBindings && ts.isNamedImports(importClause.namedBindings)) {
    for (const element of importClause.namedBindings.elements) {
      if (element.isTypeOnly) {
        continue;
      }
      const importName = (element.propertyName ?? element.name).getText(sourceFile);
      const localName = element.name.getText(sourceFile);
      bindings.push(importName === localName ? localName : `${importName}: ${localName}`);
    }
  }

  if (bindings.length === 0) {
    return 'void 0;';
  }

  return `const { ${bindings.join(', ')} } = await import(${moduleText});`;
}

function countNewlines(value: string): number {
  let count = 0;
  for (const char of value) {
    if (char === '\n') {
      count += 1;
    }
  }
  return count;
}

function parseToggleValue(rawValue: string): boolean | undefined {
  const normalized = rawValue.trim().toLowerCase();
  if (['on', 'true', '1', 'yes'].includes(normalized)) return true;
  if (['off', 'false', '0', 'no'].includes(normalized)) return false;
  return undefined;
}
