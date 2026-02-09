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
    const parsed = shellQuote.parse(afterDot);
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
      const values = await this.completeArgValue(expectingValueFor, currentToken);
      return [values, currentToken];
    }

    if (currentToken.startsWith('-')) {
      const flags = completeFlags(command, usedArgIds, currentToken);
      return [flags, currentToken];
    }

    const nextPositional = command.positionalArgs[positionalValues.length];
    if (currentToken.length > 0 && nextPositional) {
      const values = await this.completeArgValue(nextPositional, currentToken);
      return [values, currentToken];
    }

    const positionalValuesList = nextPositional
      ? await this.completeArgValue(nextPositional, currentToken)
      : [];
    const flagValuesList = completeFlags(command, usedArgIds, currentToken);
    const completions = mergeUnique(positionalValuesList, flagValuesList);
    return [completions, currentToken];
  }

  private async runReplCliCommand(command: ReplCliCommand, rawArgs: string): Promise<void> {
    const args = shellQuote
      .parse((rawArgs ?? '').trim())
      .filter((t): t is string => typeof t === 'string' && t.length > 0);

    await this.cli.run({ args: command.commandPath.concat(args), mode: 'inherit' });
  }

  private async completeArgValue(arg: CliArgMetadata, currentToken: string): Promise<string[]> {
    if (arg.possibleValues.length > 0) {
      return filterByPrefix(
        arg.possibleValues.map((value) => value.name),
        currentToken,
      );
    }

    const hook = findArgCompletionHook(arg);
    if (!hook) return [];

    return hook(this.cli, currentToken);
  }
}

type CompletionHookId = string;
type CompletionHook = (cli: GolemCli, currentToken: string) => Promise<string[]>;

const COMPLETION_HOOKS: Record<CompletionHookId, CompletionHook> = {
  AGENT_ID: async (cli, currentToken) => {
    const result = await cli.runJson({ args: ['agent', 'list'] });

    if (!result.ok || !result.json || !Array.isArray(result.json.workers)) {
      return [];
    }

    const values = result.json.workers
      .map((worker: any) => worker.workerName)
      .filter((value: unknown): value is string => typeof value === 'string');

    return filterByPrefix(values, currentToken);
  },

  COMPONENT_NAME: async (cli, currentToken) => {
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
};

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
  const leafCommands = collectLeafCommands(root);
  const commands: ReplCliCommand[] = [];

  for (const command of leafCommands) {
    const replCommand = commandPathToReplCommandName(command.path);
    const about = command.about ?? command.longAbout ?? command.name;
    const { flagArgs, positionalArgs } = indexArgs(command.args);

    commands.push({
      replCommand,
      commandPath: command.path,
      about,
      args: command.args,
      flagArgs,
      positionalArgs,
    });
  }

  return commands.sort((left, right) => left.replCommand.localeCompare(right.replCommand));
}

function collectLeafCommands(command: CliCommandMetadata): CliCommandMetadata[] {
  if (command.subcommands.length === 0) {
    return [command];
  }

  return command.subcommands.flatMap((subcommand) => collectLeafCommands(subcommand));
}

function indexArgs(args: CliArgMetadata[]): {
  flagArgs: Map<string, CliArgMetadata>;
  positionalArgs: CliArgMetadata[];
} {
  const flagArgs = new Map<string, CliArgMetadata>();
  const positionalArgs = args
    .filter((arg) => arg.isPositional)
    .sort((left, right) => (left.index ?? 0) - (right.index ?? 0));

  for (const arg of args) {
    if (arg.isPositional) continue;
    if (arg.long.length > 0) {
      for (const long of arg.long) {
        flagArgs.set(`--${long}`, arg);
      }
    } else {
      for (const short of arg.short) {
        flagArgs.set(`-${short}`, arg);
      }
    }
  }

  return { flagArgs, positionalArgs };
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

function findArgCompletionHook(arg: CliArgMetadata): CompletionHook | undefined {
  const candidates = [arg.id, ...arg.valueNames];
  for (const candidate of candidates) {
    let hook = COMPLETION_HOOKS[candidate.toUpperCase()];
    if (hook) return hook;
  }
  return;
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
