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

import childProcess from 'node:child_process';
import repl from 'node:repl';
import shellQuote from 'shell-quote';
import { CliArgMetadata, CliCommandMetadata, CliCommandsConfig, ClientConfig } from './config';

export class CliReplInterop {
  private readonly config: CliCommandsConfig;
  private readonly commands: ReplCliCommand[];
  private readonly commandsByName: Map<string, ReplCliCommand>;
  private readonly cli: GolemCli;

  constructor(config: CliCommandsConfig) {
    this.config = config;
    this.commands = collectReplCliCommands(this.config.commandMetadata);
    this.commandsByName = new Map(this.commands.map((command) => [command.replCommand, command]));
    this.cli = new GolemCli({
      binary: 'golem', // TODO: from config
      cwd: this.config.appMainDir,
      clientConfig: this.config.clientConfig,
    });
  }

  defineCommands(replServer: repl.REPLServer) {
    const interop = this;
    for (const command of this.commands) {
      replServer.defineCommand(command.replCommand, {
        help: command.about,
        async action(rawArgs: string) {
          this.pause();

          await interop.runReplCliCommand(command, rawArgs);

          this.resume();
          this.displayPrompt();
          this.clearBufferedCommand();
        },
      });
    }
  }

  async complete(line: string): Promise<[string[], string] | undefined> {
    const trimmed = line.trimStart();
    if (!trimmed.startsWith('.')) return;

    const afterDot = trimmed.slice(1);
    const endsWithSpace = /\s$/.test(line);
    const quoteInfo = detectSingleQuoteInfo(afterDot, endsWithSpace);
    const parsed = shellQuote.parse(quoteInfo.balancedInput);
    const tokens = parsed.filter((t): t is string => typeof t === 'string');
    const currentToken = endsWithSpace ? '' : tokens.length > 0 ? tokens[tokens.length - 1] : '';
    const consumedTokens = endsWithSpace ? tokens : tokens.slice(0, -1);

    if (consumedTokens.length === 0) {
      const prefix = `.${currentToken}`;
      const completions = filterByPrefix(
        [...this.commandsByName.keys()].map((name) => `.${name}`),
        prefix,
      );
      if (completions.length === 0) return;
      return [completions, prefix];
    }

    const commandName = consumedTokens[0];
    const command = this.commandsByName.get(commandName);
    if (!command) return;

    const argTokens = consumedTokens.slice(1);
    const { usedArgIds, positionalValues, expectingValueFor } = parseArgs(command, argTokens);

    if (expectingValueFor) {
      const result = await this.completeArgValue(expectingValueFor, currentToken, quoteInfo);
      return [result.values, result.completeOn];
    }

    if (currentToken.startsWith('-')) {
      const flags = completeFlags(command, usedArgIds, currentToken);
      return [flags, currentToken];
    }

    const nextPositional = command.positionalArgs[positionalValues.length];
    if (currentToken.length > 0 && nextPositional) {
      const result = await this.completeArgValue(nextPositional, currentToken, quoteInfo);
      return [result.values, result.completeOn];
    }

    const positionalValuesList = nextPositional
      ? (await this.completeArgValue(nextPositional, currentToken, quoteInfo)).values
      : [];
    const flagValuesList = completeFlags(command, usedArgIds, currentToken);
    const completions = mergeUnique(positionalValuesList, flagValuesList);
    return [completions, currentToken];
  }

  static exitWithReloadCode() {
    process.exit(75);
  }

  private async runReplCliCommand(
    command: ReplCliCommand,
    rawArgs: string,
  ): Promise<{
    ok: boolean;
    code: number | null;
  }> {
    let args = shellQuote
      .parse((rawArgs ?? '').trim())
      .filter((t): t is string => typeof t === 'string' && t.length > 0);

    const hook = COMMAND_HOOKS[command.replCommand];

    if (hook) {
      args = hook.adaptArgs(args);
    }

    let result = await this.cli.run({ args: command.commandPath.concat(args), mode: 'inherit' });

    if (hook) {
      hook.handleResult(command.commandPath.concat(args), result);
    }

    return result;
  }

  private async completeArgValue(
    arg: CliArgMetadata,
    currentToken: string,
    quoteInfo: SingleQuoteInfo,
  ): Promise<{ values: string[]; completeOn: string }> {
    const isSingleQuoted = quoteInfo.isSingleQuoted;
    const tokenForCompletion = isSingleQuoted ? quoteInfo.unquotedToken : currentToken;

    if (arg.possibleValues.length > 0) {
      const values = filterByPrefix(
        arg.possibleValues.map((value) => value.name),
        tokenForCompletion,
      );
      if (isSingleQuoted && !quoteInfo.isClosed) {
        return {
          values: applySingleQuoteCompletionsOpen(values),
          completeOn: '',
        };
      }
      return {
        values: isSingleQuoted ? applySingleQuoteCompletions(values) : values,
        completeOn: isSingleQuoted ? quoteInfo.rawToken : currentToken,
      };
    }

    const hook = findArgCompletionHook(arg);
    if (!hook) {
      return { values: [], completeOn: isSingleQuoted ? quoteInfo.rawToken : currentToken };
    }

    const requiresSingleQuotes = hook.requiresSingleQuotes ?? false;
    const useSingleQuotes = isSingleQuoted || requiresSingleQuotes;
    const values = await hook.complete(this.cli, tokenForCompletion);
    return {
      values: useSingleQuotes
        ? isSingleQuoted && !quoteInfo.isClosed
          ? applySingleQuoteCompletionsOpen(values)
          : applySingleQuoteCompletions(values)
        : values,
      completeOn:
        isSingleQuoted && !quoteInfo.isClosed
          ? ''
          : isSingleQuoted
            ? quoteInfo.rawToken
            : currentToken,
    };
  }
}

type CommandHookId = string;
type CommandHook = {
  adaptArgs: (args: string[]) => string[];
  handleResult: (args: string[], result: { ok: boolean; code: number | null }) => void;
};

const COMMAND_HOOKS: Partial<Record<CommandHookId, CommandHook>> = {
  deploy: {
    adaptArgs: (args) => ['--repl-bridge-sdk-target', 'ts', ...args],
    handleResult: (args, result) => {
      if (args.includes('--plan') || args.includes('stage')) return;
      if (!result.ok) return;
      CliReplInterop.exitWithReloadCode();
    },
  },
};

type CompletionHookId = string;
type CompletionHookFn = (cli: GolemCli, currentToken: string) => Promise<string[]>;
type CompletionHook = { complete: CompletionHookFn; requiresSingleQuotes?: boolean };

const COMPLETION_HOOKS: Partial<Record<CompletionHookId, CompletionHook>> = {
  AGENT_ID: {
    requiresSingleQuotes: true,
    complete: async (cli, currentToken) => {
      const result = await cli.runJson({ args: ['agent', 'list'] });

      if (!result.ok || !result.json || !Array.isArray(result.json.workers)) {
        return [];
      }

      const values = result.json.workers
        .map((worker: any) => worker.workerName)
        .filter((value: unknown): value is string => typeof value === 'string');

      return filterByPrefix(values, currentToken);
    },
  },

  COMPONENT_NAME: {
    complete: async (cli, currentToken) => {
      const result = await cli.runJson({ args: ['component', 'list'] });
      if (!result.ok) {
        return [];
      }

      if (!result.json || !Array.isArray(result.json)) {
        return [];
      }

      const values = result.json
        .map((component: any) => component?.componentName)
        .filter((value: unknown): value is string => typeof value === 'string');

      return filterByPrefix(values, currentToken);
    },
  },
};

function findArgCompletionHook(arg: CliArgMetadata): CompletionHook | undefined {
  const candidates = [arg.id, ...arg.valueNames];
  for (const candidate of candidates) {
    const hook = COMPLETION_HOOKS[candidate];
    if (hook) return hook;
  }
  return;
}

class GolemCli {
  private readonly binaryName: string;
  private readonly cwd: string;
  private readonly clientConfig: ClientConfig;

  constructor(opts: { binary: string; cwd: string; clientConfig: ClientConfig }) {
    this.binaryName = opts.binary;
    this.cwd = opts.cwd;
    this.clientConfig = opts.clientConfig;
  }

  async run(opts: { args: string[]; mode: 'inherit' | 'piped' }): Promise<{
    ok: boolean;
    code: number | null;
    stdout: string;
    stderr: string;
  }> {
    const child = childProcess.spawn(
      this.binaryName,
      ['--environment', this.clientConfig.environment, ...opts.args],
      {
        cwd: this.cwd,
        stdio: ((mode) => {
          switch (mode) {
            case 'inherit':
              return 'inherit';
            case 'piped':
              return ['ignore', 'pipe', 'pipe'];
          }
        })(opts.mode),
      },
    );

    return new Promise((resolve) => {
      let stdout = '';
      let stderr = '';

      if (opts.mode === 'piped') {
        child.stdout?.on('data', (chunk) => {
          stdout += chunk.toString();
        });

        child.stderr?.on('data', (chunk) => {
          stderr += chunk.toString();
        });
      }

      child.once('exit', (code) => {
        resolve({ ok: code === 0, code, stdout, stderr });
      });
    });
  }

  async runJson(opts: {
    args: string[];
  }): Promise<{ ok: boolean; code: number | null; json: any }> {
    const result = await this.run({ args: ['--format', 'json', ...opts.args], mode: 'piped' });
    return { ok: result.ok, code: result.code, json: JSON.parse(result.stdout) };
  }
}

type ReplCliCommand = {
  replCommand: string;
  commandPath: string[];
  about: string;
  args: CliArgMetadata[];
  flagArgs: Map<string, CliArgMetadata>;
  positionalArgs: CliArgMetadata[];
};

function collectReplCliCommands(root: CliCommandMetadata): ReplCliCommand[] {
  const commands: ReplCliCommand[] = [];

  function collect(
    parentGlobalFlagsArgs: Map<string, CliArgMetadata>,
    command: CliCommandMetadata,
  ) {
    const replCommand = commandPathToReplCommandName(command.path);
    const about = command.about ?? command.longAbout ?? command.name;
    const { globalFlagArgs, flagArgs, positionalArgs } = partitionArgs(command.args);

    flagArgs.set('--help', {
      action: '',
      defaultValues: [],
      id: 'help',
      isGlobal: false,
      isHidden: false,
      isPositional: false,
      isRequired: false,
      long: [],
      possibleValues: [],
      short: [],
      takesValue: false,
      valueHint: '',
      valueNames: [],
    });

    for (let [flagName, flagArg] of parentGlobalFlagsArgs) {
      flagArgs.set(flagName, flagArg);
    }

    const subcommandGlobalFlagArgs =
      globalFlagArgs.size === 0
        ? parentGlobalFlagsArgs
        : new Map([...parentGlobalFlagsArgs, ...globalFlagArgs]);

    if (command.subcommands.length === 0) {
      commands.push({
        replCommand,
        commandPath: command.path,
        about,
        args: command.args,
        flagArgs,
        positionalArgs,
      });
    } else {
      for (const subcommand of command.subcommands) {
        collect(subcommandGlobalFlagArgs, subcommand);
      }
    }
  }

  collect(new Map(), root);

  return commands.sort((left, right) => left.replCommand.localeCompare(right.replCommand));
}

function partitionArgs(args: CliArgMetadata[]): {
  globalFlagArgs: Map<string, CliArgMetadata>;
  flagArgs: Map<string, CliArgMetadata>;
  positionalArgs: CliArgMetadata[];
} {
  const globalFlagArgs = new Map<string, CliArgMetadata>();
  const flagArgs = new Map<string, CliArgMetadata>();

  const positionalArgs = args
    .filter((arg) => arg.isPositional)
    .sort((left, right) => (left.index ?? 0) - (right.index ?? 0));

  for (const arg of args) {
    if (arg.isPositional) continue;

    // TODO: either fix clap extraction, or patch it in golem bin
    if (arg.id === 'format') {
      arg.possibleValues = [
        {
          name: 'text',
          hidden: false,
          aliases: [],
        },
        {
          name: 'json',
          hidden: false,
          aliases: [],
        },
        {
          name: 'yaml',
          hidden: false,
          aliases: [],
        },
        {
          name: 'pretty-json',
          hidden: false,
          aliases: [],
        },
        {
          name: 'pretty-yaml',
          hidden: false,
          aliases: [],
        },
      ];
    }

    if (arg.long.length > 0) {
      for (const long of arg.long) {
        flagArgs.set(`--${long}`, arg);
        if (arg.isGlobal) {
          globalFlagArgs.set(`--${long}`, arg);
        }
      }
    } else {
      for (const short of arg.short) {
        flagArgs.set(`-${short}`, arg);
        if (arg.isGlobal) {
          globalFlagArgs.set(`-${short}`, arg);
        }
      }
    }
  }

  return { globalFlagArgs, flagArgs, positionalArgs };
}

function parseArgs(command: ReplCliCommand, tokens: string[]) {
  const usedArgIds = new Set<string>();
  const positionalValues: string[] = [];
  let expectingValueFor: CliArgMetadata | undefined;

  for (let index = 0; index < tokens.length; index++) {
    const token = tokens[index];
    if (expectingValueFor) {
      expectingValueFor = undefined;
      continue;
    }

    if (token === '--') {
      positionalValues.push(...tokens.slice(index + 1));
      break;
    }

    const flagArg = command.flagArgs.get(token);
    if (flagArg) {
      usedArgIds.add(flagArg.id);
      if (flagArg.takesValue) {
        expectingValueFor = flagArg;
      }
      continue;
    }

    positionalValues.push(token);
  }

  return { usedArgIds, positionalValues, expectingValueFor };
}

function completeFlags(command: ReplCliCommand, usedArgIds: Set<string>, prefix: string): string[] {
  const flags: string[] = [];
  for (const [flag, arg] of command.flagArgs.entries()) {
    const allowMultiple = arg.action === 'Append' || arg.action === 'Count';
    if (!allowMultiple && usedArgIds.has(arg.id)) continue;
    flags.push(flag);
  }

  return filterByPrefix(flags, prefix);
}

function filterByPrefix(values: string[], prefix: string): string[] {
  if (!prefix) return Array.from(new Set(values));
  return Array.from(new Set(values.filter((value) => value.startsWith(prefix))));
}

function mergeUnique(left: string[], right: string[]): string[] {
  const set = new Set<string>();
  left.forEach((value) => set.add(value));
  right.forEach((value) => set.add(value));
  return [...set];
}

function commandPathToReplCommandName(segments: string[]): string {
  const parts = segments
    .flatMap((segment) => segment.split(/[-_]/g))
    .filter((segment) => segment.length > 0);
  return parts
    .map((part, index) => {
      if (index === 0) return part.toLowerCase();
      return part[0].toUpperCase() + part.slice(1).toLowerCase();
    })
    .join('');
}

type SingleQuoteInfo = {
  balancedInput: string;
  rawToken: string;
  unquotedToken: string;
  isSingleQuoted: boolean;
  isClosed: boolean;
};

function detectSingleQuoteInfo(input: string, endsWithSpace: boolean): SingleQuoteInfo {
  const matchInput = endsWithSpace ? input.trimEnd() : input;
  const quotedMatch = matchInput.match(/'(?:\\'|[^'])*'?$/);
  const wordMatch = matchInput.match(/(?:^|\s)([^\s]*)$/);
  const rawToken = quotedMatch?.[0] ?? wordMatch?.[1] ?? '';
  const isSingleQuoted = rawToken.startsWith("'");
  const isClosed =
    isSingleQuoted &&
    rawToken.length > 1 &&
    rawToken.endsWith("'") &&
    !isEscaped(rawToken, rawToken.length - 1);
  const unquotedToken = isSingleQuoted
    ? unescapeSingleQuotes(rawToken.slice(1, isClosed ? -1 : rawToken.length))
    : rawToken;

  return {
    balancedInput: isSingleQuoted && !isClosed ? `${input}'` : input,
    rawToken,
    unquotedToken,
    isSingleQuoted,
    isClosed,
  };
}

function isEscaped(value: string, index: number): boolean {
  let count = 0;
  for (let i = index - 1; i >= 0; i--) {
    if (value[i] !== '\\') break;
    count++;
  }
  return count % 2 === 1;
}

function unescapeSingleQuotes(value: string): string {
  return value.replace(/\\'/g, "'");
}

function applySingleQuoteCompletions(values: string[]): string[] {
  if (values.length === 1) {
    return [`'${escapeSingleQuotes(values[0])}'`];
  }
  return values.map((value) => `'${escapeSingleQuotes(value)}`);
}

function applySingleQuoteCompletionsOpen(values: string[]): string[] {
  if (values.length === 1) {
    return [`${escapeSingleQuotes(values[0])}'`];
  }
  return values.map((value) => escapeSingleQuotes(value));
}

function escapeSingleQuotes(value: string): string {
  return value.replace(/'/g, "\\'");
}
