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
import fs from 'node:fs';
import repl from 'node:repl';
import shellQuote from 'shell-quote';
import { CliCommandsConfig, ClientConfig } from './config';

export type CliCommandMetadata = {
  path: string[];
  name: string;
  displayName?: string | null;
  about?: string | null;
  longAbout?: string | null;
  hidden: boolean;
  visibleAliases: string[];
  args: CliArgMetadata[];
  subcommands: CliCommandMetadata[];
};

export type CliArgMetadata = {
  id: string;
  help?: string | null;
  longHelp?: string | null;
  valueNames: string[];
  valueHint: string;
  possibleValues: CliPossibleValueMetadata[];
  action: string;
  numArgs?: string | null;
  isPositional: boolean;
  isRequired: boolean;
  isGlobal: boolean;
  isHidden: boolean;
  index?: number | null;
  long: string[];
  short: string[];
  defaultValues: string[];
  takesValue: boolean;
};

export type CliPossibleValueMetadata = {
  name: string;
  help?: string | null;
  hidden: boolean;
  aliases: string[];
};

type CompletionHookId = string;

type CompletionContext = {
  cwd: string;
  clientConfig: ClientConfig;
  currentToken: string;
};

type CompletionHook = (context: CompletionContext) => Promise<string[]>;

type ReplCliCommand = {
  replCommand: string;
  commandPath: string[];
  about: string;
  args: CliArgMetadata[];
  flagArgs: Map<string, CliArgMetadata>;
  positionalArgs: CliArgMetadata[];
};

const DEFAULT_ARG_COMPLETION_HOOKS: Record<string, CompletionHookId> = {
  agent_id: 'agentId',
  AGENT_ID: 'agentId',
  component_name: 'componentName',
  COMPONENT_NAME: 'componentName',
};

const COMPLETION_HOOKS: Record<CompletionHookId, CompletionHook> = {
  agentId: async ({ cwd, clientConfig, currentToken }) => {
    const currentTokenStartsWithSingleQuote = currentToken.startsWith("'");
    const rawCurrentToken = currentTokenStartsWithSingleQuote
      ? currentToken.slice(1, -1)
      : currentToken;

    const json = await runGolemJson(cwd, [
      '--format',
      'json',
      '--environment',
      clientConfig.environment,
      'agent',
      'list',
    ]);

    if (!json || !Array.isArray(json.workers)) {
      return [];
    }

    const values = json.workers
      .map((worker: any) => worker.workerName)
      .filter((value: unknown): value is string => typeof value === 'string');

    let matches = filterByPrefix(values, rawCurrentToken);
    if (currentTokenStartsWithSingleQuote) {
      if (matches.length === 0) {
        matches = matches.map((match) => `'${match}'`);
      } else {
        matches = matches.map((match) => `'${match}'`);
      }
    }
    return matches;
  },
  componentName: async ({ cwd, clientConfig, currentToken }) => {
    const json = await runGolemJson(cwd, [
      '--format',
      'json',
      '--environment',
      clientConfig.environment,
      'component',
      'list',
    ]);
    if (!Array.isArray(json)) {
      return [];
    }

    const values = json
      .map((component: any) => component?.componentName)
      .filter((value: unknown): value is string => typeof value === 'string');

    return filterByPrefix(values, currentToken);
  },
};

export class CliCommands {
  private readonly config: CliCommandsConfig;
  private readonly commands: ReplCliCommand[];
  private readonly commandsByName: Map<string, ReplCliCommand>;

  constructor(config: CliCommandsConfig) {
    this.config = config;
    this.commands = addReplCommands(this.config.commandMetadata);
    this.commandsByName = new Map(this.commands.map((command) => [command.replCommand, command]));
  }

  defineCommands(replServer: repl.REPLServer) {
    const reserved = new Set(['deploy', 'reload']);
    const clientConfig = this.config.clientConfig;
    const appMainDir = this.config.appMainDir;
    for (const command of this.commands) {
      if (reserved.has(command.replCommand)) continue;

      const help = command.about || `Run golem ${command.commandPath.join(' ')}`;
      replServer.defineCommand(command.replCommand, {
        help,
        async action(rawArgs: string) {
          this.pause();

          await runReplCliCommand(command, appMainDir, rawArgs, clientConfig);

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
      const values = await completeArgValue(
        expectingValueFor,
        currentToken,
        this.config.appMainDir,
        this.config.clientConfig,
      );
      return [values, currentToken];
    }

    if (currentToken.startsWith('-')) {
      const flags = completeFlags(command, usedArgIds, currentToken);
      return [flags, currentToken];
    }

    const nextPositional = command.positionalArgs[positionalValues.length];
    if (currentToken.length > 0 && nextPositional) {
      const values = await completeArgValue(
        nextPositional,
        currentToken,
        this.config.appMainDir,
        this.config.clientConfig,
      );
      return [values, currentToken];
    }

    const positionalValuesList = nextPositional
      ? await completeArgValue(
          nextPositional,
          currentToken,
          this.config.appMainDir,
          this.config.clientConfig,
        )
      : [];
    const flagValuesList = completeFlags(command, usedArgIds, currentToken);
    const completions = mergeUnique(positionalValuesList, flagValuesList);
    return [completions, currentToken];
  }
}

function addReplCommands(root: CliCommandMetadata): ReplCliCommand[] {
  const leafCommands = collectLeafCommands(root);
  const commands: ReplCliCommand[] = [];

  for (const command of leafCommands) {
    const replCommand = toLowerCamelCase(command.path);
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

  return commands;
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

async function completeArgValue(
  arg: CliArgMetadata,
  currentToken: string,
  cwd: string,
  clientConfig: ClientConfig,
): Promise<string[]> {
  if (arg.possibleValues.length > 0) {
    return filterByPrefix(
      arg.possibleValues.map((value) => value.name),
      currentToken,
    );
  }

  const hookId = resolveCompletionHookId(arg);
  if (!hookId) return [];

  const hook = COMPLETION_HOOKS[hookId];
  if (!hook) return [];

  return hook({ cwd, clientConfig, currentToken });
}

function resolveCompletionHookId(arg: CliArgMetadata): CompletionHookId | undefined {
  const candidates = [arg.id, ...arg.valueNames];
  for (const candidate of candidates) {
    const hookId = DEFAULT_ARG_COMPLETION_HOOKS[candidate];
    if (hookId) return hookId;
    const upper = candidate.toUpperCase();
    if (DEFAULT_ARG_COMPLETION_HOOKS[upper]) {
      return DEFAULT_ARG_COMPLETION_HOOKS[upper];
    }
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

function toLowerCamelCase(segments: string[]): string {
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

async function runReplCliCommand(
  command: ReplCliCommand,
  cwd: string,
  rawArgs: string,
  clientConfig: ClientConfig,
): Promise<void> {
  const parsedArgs = shellQuote.parse((rawArgs ?? '').trim());
  const args = parsedArgs.filter((t): t is string => typeof t === 'string' && t.length > 0);

  const cliArgs = ['--environment', clientConfig.environment, ...command.commandPath, ...args];
  const child = childProcess.spawn('golem', cliArgs, {
    stdio: 'inherit',
    cwd,
  });
  await new Promise<void>((resolve) =>
    child.once('exit', () => {
      resolve();
    }),
  );
}

async function runGolemJson(cwd: string, args: string[]): Promise<any> {
  const result = await runGolem(cwd, args);
  if (!result.ok) return;
  try {
    return JSON.parse(result.stdout);
  } catch {
    return;
  }
}

async function runGolem(
  cwd: string,
  args: string[],
): Promise<{ ok: boolean; stdout: string; stderr: string }> {
  const child = childProcess.spawn('golem', args, {
    cwd,
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  return new Promise((resolve) => {
    let stdout = '';
    let stderr = '';

    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString();
    });

    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString();
    });

    child.once('exit', (code) => {
      resolve({ ok: code === 0, stdout, stderr });
    });
  });
}
