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

import childProcess, { ChildProcess } from 'node:child_process';
import repl from 'node:repl';
import pc from 'picocolors';
import { CliArgMetadata, CliCommandMetadata, CliCommandsConfig } from './config';
import { flushStdIO, writeChunk } from './process';
import { writeFullLineSeparator } from './format';
import type * as base from '@golemcloud/golem-ts-bridge';
import * as uuid from 'uuid';

const AGENT_STREAM_CLOSE_DELAY_MS = 300;

export class CliReplInterop {
  private readonly config: CliCommandsConfig;
  private readonly commands: ReplCliCommand[];
  private readonly commandsByName: Map<string, ReplCliCommand>;
  private readonly cli: GolemCli;
  private builtinCommands: string[];
  private readonly agentStreams: Map<string, AgentStreamState>;
  private replServer: repl.REPLServer | undefined;

  constructor(config: CliCommandsConfig) {
    this.config = config;
    this.commands = collectReplCliCommands(this.config.commandMetadata);
    this.commandsByName = new Map(this.commands.map((command) => [command.replCommand, command]));
    this.cli = new GolemCli({
      binary: config.binary,
      cwd: this.config.appMainDir,
      clientConfig: this.config.clientConfig,
    });
    this.builtinCommands = [];
    this.agentStreams = new Map();
  }

  defineCommands(replServer: repl.REPLServer) {
    this.replServer = replServer;
    this.builtinCommands = Object.keys(replServer.commands);

    // Wrap the built-in onSigInt listener so we can suppress the
    // "To exit, press Ctrl+C again or Ctrl+D or type .exit" message when
    // Ctrl-C cancels a running CLI command rather than acting as a bare keypress.
    let suppressNextSigint = false;
    const [builtinOnSigInt] = replServer.rawListeners('SIGINT') as [
      ((...args: unknown[]) => void) | undefined,
    ];
    if (builtinOnSigInt) {
      replServer.removeAllListeners('SIGINT');
      replServer.on('SIGINT', function wrapper() {
        if (suppressNextSigint) {
          suppressNextSigint = false;
          return;
        }
        builtinOnSigInt.call(replServer);
      });
    }

    const interop = this;
    for (const command of this.commands) {
      replServer.defineCommand(command.replCommand, {
        help: command.about,
        async action(rawArgs: string) {
          // Do not call this.pause() — it blocks stdin which prevents the readline
          // Interface from emitting 'SIGINT' events when the user presses Ctrl-C.
          // Instead we suppress concurrent eval for the duration of the command.
          const abortController = new AbortController();
          suppressNextSigint = true;
          const onSigint = () => abortController.abort();
          replServer.once('SIGINT', onSigint);

          const savedEval = (replServer as any).eval;
          (replServer as any).eval = (_: string, __: unknown, ___: string, cb: (err: Error | null, result?: unknown) => void) => cb(null, undefined);

          try {
            await interop.runReplCliCommand(command, rawArgs, abortController.signal);
          } finally {
            suppressNextSigint = false;
            (replServer as any).eval = savedEval;
            replServer.off('SIGINT', onSigint);
            this.displayPrompt();
            this.clearBufferedCommand();
          }
        },
      });
    }
  }

  async complete(line: string): Promise<[string[], string] | undefined> {
    let startTrimmed = line.trimStart();
    const prefixChar = startTrimmed[0];
    if (prefixChar !== '.' && prefixChar !== ':') return;
    startTrimmed = startTrimmed.slice(1);

    const endsWithSpace = /\s$/.test(line);
    const tokens = parseRawArgs(startTrimmed);

    const builtinCompletions =
      tokens.length === 1
        ? this.builtinCommands
            .filter((command) => command.startsWith(tokens[0]))
            .map((command) => `${prefixChar}${command}`)
        : [];

    const lastToken = tokens.length > 0 ? tokens[tokens.length - 1] : '';
    const endsWithSeparator = endsWithSpace && !/\s$/.test(lastToken);

    const currentToken = endsWithSeparator
      ? ''
      : tokens.length > 0
        ? tokens[tokens.length - 1]
        : '';
    const consumedTokens = endsWithSeparator ? tokens : tokens.slice(0, -1);

    if (consumedTokens.length === 0) {
      const prefix = `${prefixChar}${currentToken}`;
      const completions = filterByPrefix(
        [...this.commandsByName.keys()].map((name) => `${prefixChar}${name}`),
        prefix,
      );
      const allCompletions = [...builtinCompletions, ...completions];
      if (allCompletions.length === 0) return;
      return [allCompletions, prefix];
    }

    const commandName = consumedTokens[0];
    const command = this.commandsByName.get(commandName);
    if (!command) return;

    const argTokens = consumedTokens.slice(1);
    const { usedArgIds, positionalValues, expectingValueFor } = parseArgs(command, argTokens);

    if (expectingValueFor) {
      const result = await this.completeArgValue(expectingValueFor, currentToken);
      return [result.values, result.completeOn];
    }

    if (currentToken.startsWith('-')) {
      const flags = completeFlags(command, usedArgIds, currentToken);
      return [flags, currentToken];
    }

    const nextPositional = command.positionalArgs[positionalValues.length];
    if (currentToken.length > 0 && nextPositional) {
      const result = await this.completeArgValue(nextPositional, currentToken);
      return [result.values, result.completeOn];
    }

    const positionalValuesList = nextPositional
      ? (await this.completeArgValue(nextPositional, currentToken)).values
      : [];
    const flagValuesList = completeFlags(command, usedArgIds, currentToken);
    const completions = mergeUnique(positionalValuesList, flagValuesList);
    return [completions, currentToken];
  }

  static async exitWithReloadCode() {
    await flushStdIO();
    process.exit(75);
  }

  startAgentStream(request: base.AgentInvocationRequest) {
    const key = getAgentStreamKey(request);
    if (this.agentStreams.has(key)) {
      return;
    }

    const parameters = safeJsonStringify(request.parameters);

    const idempotencyKey = request.idempotencyKey ?? uuid.v4().toString();
    if (!request.idempotencyKey) {
      request.idempotencyKey = idempotencyKey;
    }

    const args = [
      '--environment',
      this.config.clientConfig.environment,
      'agent',
      'repl-stream',
      '--quiet',
      '--logs-only',
      request.agentTypeName,
      parameters,
      idempotencyKey,
    ];
    if (request.phantomId) {
      args.push(request.phantomId);
    }

    const child = childProcess.spawn(this.config.binary, args, {
      cwd: this.config.appMainDir,
      stdio: ['pipe', 'pipe', 'pipe'],
      env: {
        ...process.env,
        CLICOLOR_FORCE: pc.isColorSupported ? '1' : '0',
      },
    });

    const onSigint = () => {
      void this.stopAgentStreamByKey(key, true);
    };
    const state = createAgentStreamState(child, onSigint);
    this.agentStreams.set(key, state);
    this.replServer?.once('SIGINT', onSigint);

    child.stdout?.on('data', state.onStdout);
    child.stderr?.on('data', state.onStderr);

    child.once('error', () => {
      void this.stopAgentStreamByKey(key);
    });
    child.once('exit', () => {
      void this.stopAgentStreamByKey(key);
    });
  }

  async stopAgentStream(request: base.AgentInvocationRequest) {
    const key = getAgentStreamKey(request);
    await this.stopAgentStreamByKey(key);
  }

  private async stopAgentStreamByKey(key: string, immediate = false) {
    const state = this.agentStreams.get(key);
    if (!state) return;
    if (!immediate) {
      await new Promise((resolve) => setTimeout(resolve, AGENT_STREAM_CLOSE_DELAY_MS));
      if (this.agentStreams.get(key) !== state) return;
    }
    this.agentStreams.delete(key);
    this.replServer?.off('SIGINT', state.onSigint);
    state.stop();
    if (!immediate && state.hadOutput()) {
      writeFullLineSeparator();
    }
  }

  private async runReplCliCommand(
    command: ReplCliCommand,
    rawArgs: string,
    signal?: AbortSignal,
  ): Promise<{
    ok: boolean;
    code: number | null;
  }> {
    let args = parseRawArgs(rawArgs);

    const hook = COMMAND_HOOKS[command.replCommand];

    if (hook) {
      args = hook.adaptArgs(args);
    }

    let result = await this.cli.run({ args: command.commandPath.concat(args), mode: 'inherit', signal });

    if (hook) {
      await hook.handleResult(command.commandPath.concat(args), result);
    }

    return result;
  }

  private async completeArgValue(
    arg: CliArgMetadata,
    currentToken: string,
  ): Promise<{ values: string[]; completeOn: string }> {
    if (arg.possibleValues.length > 0) {
      const values = filterByPrefix(
        arg.possibleValues.map((value) => value.name),
        currentToken,
      );
      return {
        values,
        completeOn: currentToken,
      };
    }

    const hook = findArgCompletionHook(arg);
    if (!hook) {
      return { values: [], completeOn: currentToken };
    }

    const values = await hook.complete(this.cli, currentToken);
    return {
      values,
      completeOn: currentToken,
    };
  }
}

type CommandHookId = string;
type CommandHook = {
  adaptArgs: (args: string[]) => string[];
  handleResult: (args: string[], result: { ok: boolean; code: number | null }) => Promise<void>;
};

type AgentStreamState = {
  stop: () => void;
  onStdout: (chunk: Buffer) => void;
  onStderr: (chunk: Buffer) => void;
  hadOutput: () => boolean;
  onSigint: () => void;
};

function createAgentStreamState(child: ChildProcess, onSigint: () => void): AgentStreamState {
  let outputSeen = false;

  const onStdout = (chunk: Buffer) => {
    outputSeen = outputSeen || chunk.length > 0;
    writeChunk(chunk);
  };

  const onStderr = (chunk: Buffer) => {
    outputSeen = outputSeen || chunk.length > 0;
    process.stderr.write(chunk);
  };

  const stop = () => {
    child.stdout?.off('data', onStdout);
    child.stderr?.off('data', onStderr);
    child.removeAllListeners('error');
    child.removeAllListeners('exit');
    if (child.exitCode === null && !child.killed) {
      child.kill();
    }
  };

  return {
    stop,
    onStdout,
    onStderr,
    hadOutput: () => outputSeen,
    onSigint,
  };
}

function getAgentStreamKey(request: base.AgentInvocationRequest): string {
  return [
    request.agentTypeName,
    safeJsonStringify(request.parameters),
    request.phantomId ?? '',
  ].join('|');
}

function safeJsonStringify(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

const COMMAND_HOOKS: Partial<Record<CommandHookId, CommandHook>> = {
  deploy: {
    adaptArgs: (args) => ['--repl-bridge-sdk-target', 'ts', ...args],
    handleResult: async (args, result) => {
      if (args.includes('--plan') || args.includes('stage')) return;
      if (!result.ok) return;
      await CliReplInterop.exitWithReloadCode();
    },
  },
};

type CompletionHookId = string;
type CompletionHookFn = (cli: GolemCli, currentToken: string) => Promise<string[]>;
type CompletionHook = { complete: CompletionHookFn };

const COMPLETION_HOOKS: Partial<Record<CompletionHookId, CompletionHook>> = {
  AGENT_ID: {
    complete: async (cli, currentToken) => {
      const result = await cli.runJson({ args: ['agent', 'list'] });

      if (!result.ok || !result.json || !Array.isArray(result.json.agents)) {
        return [];
      }

      const values = result.json.agents
        .map((agent: any) => {
          const agentName = agent?.agentName;
          if (typeof agentName !== 'string') {
            return undefined;
          }
          return agentName;
        })
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
  private readonly clientConfig: base.Configuration;

  constructor(opts: { binary: string; cwd: string; clientConfig: base.Configuration }) {
    this.binaryName = opts.binary;
    this.cwd = opts.cwd;
    this.clientConfig = opts.clientConfig;
  }

  async run(opts: { args: string[]; mode: 'inherit' | 'collect'; signal?: AbortSignal }): Promise<{
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
            case 'collect':
              return ['ignore', 'pipe', 'pipe'];
          }
        })(opts.mode),
      },
    );

    return new Promise((resolve) => {
      let stdout = '';
      let stderr = '';

      if (opts.mode === 'collect') {
        child.stdout?.on('data', (chunk) => {
          stdout += chunk.toString();
        });

        child.stderr?.on('data', (chunk) => {
          stderr += chunk.toString();
        });
      }

      const onAbort = () => {
        child.kill('SIGINT');
      };
      opts.signal?.addEventListener('abort', onAbort, { once: true });

      const cleanup = (code: number | null) => {
        opts.signal?.removeEventListener('abort', onAbort);
        resolve({ ok: code === 0, code, stdout, stderr });
      };

      child.once('exit', cleanup);
      child.once('error', () => cleanup(null));
    });
  }

  async runJson(opts: {
    args: string[];
  }): Promise<{ ok: boolean; code: number | null; json: any }> {
    const result = await this.run({ args: ['--format', 'json', ...opts.args], mode: 'collect' });
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
    .filter((segment) => segment.length > 0)
    .map((part) => part.toLowerCase());
  return parts.join('-');
}

export function parseRawArgs(rawArgs: string): string[] {
  const args: string[] = [];
  let current = '';
  let inSingle = false;
  let inDouble = false;
  let escaping = false;
  let inAgent = false;
  let agentDepth = 0;
  let agentInSingle = false;
  let agentInDouble = false;
  let agentEscaping = false;

  function pushCurrent() {
    if (current.length > 0) {
      args.push(current);
      current = '';
    }
  }

  function isIdentChar(ch: string): boolean {
    return /[A-Za-z0-9_-]/.test(ch);
  }

  function tryReadAgentCallOpenParenIndex(start: number): number | undefined {
    let simpleAgentEnd = start;
    while (simpleAgentEnd < rawArgs.length && isIdentChar(rawArgs[simpleAgentEnd])) {
      simpleAgentEnd += 1;
    }

    if (simpleAgentEnd === start) {
      return;
    }

    if (rawArgs[simpleAgentEnd] === '(') {
      return simpleAgentEnd;
    }
  }

  for (let i = 0; i < rawArgs.length; i += 1) {
    const ch = rawArgs[i];

    if (inAgent) {
      current += ch;

      if (agentEscaping) {
        agentEscaping = false;
        continue;
      }

      if (agentInSingle) {
        if (ch === "'") agentInSingle = false;
        continue;
      }

      if (agentInDouble) {
        if (ch === '\\') {
          agentEscaping = true;
        } else if (ch === '"') {
          agentInDouble = false;
        }
        continue;
      }

      if (ch === "'") {
        agentInSingle = true;
        continue;
      }
      if (ch === '"') {
        agentInDouble = true;
        continue;
      }

      if (ch === '(') {
        agentDepth += 1;
      } else if (ch === ')') {
        agentDepth -= 1;
        if (agentDepth === 0) {
          inAgent = false;
        }
      }
      continue;
    }

    if (escaping) {
      current += ch;
      escaping = false;
      continue;
    }

    if (inSingle) {
      if (ch === "'") {
        inSingle = false;
      } else {
        current += ch;
      }
      continue;
    }

    if (inDouble) {
      if (ch === '\\') {
        escaping = true;
      } else if (ch === '"') {
        inDouble = false;
      } else {
        current += ch;
      }
      continue;
    }

    if (/\s/.test(ch)) {
      pushCurrent();
      continue;
    }

    if (ch === "'") {
      inSingle = true;
      continue;
    }

    if (ch === '"') {
      inDouble = true;
      continue;
    }

    if (ch === '\\') {
      escaping = true;
      continue;
    }

    if (current.length === 0 && (isIdentChar(ch) || /[A-Za-z0-9-]/.test(ch))) {
      const openParenIndex = tryReadAgentCallOpenParenIndex(i);
      if (openParenIndex !== undefined) {
        current += rawArgs.slice(i, openParenIndex);
        current += '(';
        inAgent = true;
        agentDepth = 1;
        i = openParenIndex;
        continue;
      }
    }

    current += ch;
  }

  pushCurrent();
  return args;
}
