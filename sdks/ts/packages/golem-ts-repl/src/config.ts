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

import { env } from 'node:process';
import fs from 'node:fs';
import util from 'node:util';
import * as base from './base';

export type Config = {
  binary: string;
  appMainDir: string;
  agents: Record<string, AgentConfig>;
  historyFile: string;
  cliCommandsMetadataJsonPath: string;
};

export type ReplCliFlags = {
  script?: string;
  scriptPath?: string;
  disableAutoImports: boolean;
  showTypeInfo: boolean;
  streamLogs: boolean;
};

export type AgentModule = Record<string, unknown> & {
  configure: ConfigureClient;
};

export type AgentConfig<T extends AgentModule = AgentModule> = {
  clientPackageName: string;
  clientPackageImportedName: string;
  package: T;
};

export type ConfigureClient = (config: base.Configuration) => void;

export type CliCommandsConfig = {
  binary: string;
  appMainDir: string;
  clientConfig: base.Configuration;
  commandMetadata: CliCommandMetadata;
};

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

export function clientConfigFromEnv(): base.Configuration {
  return {
    server: clientServerConfigFromEnv(),
    application: requiredEnvVar('GOLEM_REPL_APPLICATION'),
    environment: requiredEnvVar('GOLEM_REPL_ENVIRONMENT'),
  };
}

export function cliCommandsConfigFromBaseConfig(
  config: Config,
  clientConfig: base.Configuration,
): CliCommandsConfig {
  const commandMetadataContents = fs.readFileSync(config.cliCommandsMetadataJsonPath, 'utf8');
  const commandMetadata = JSON.parse(commandMetadataContents) as CliCommandMetadata;

  return {
    binary: config.binary,
    appMainDir: config.appMainDir,
    clientConfig,
    commandMetadata,
  };
}

function clientServerConfigFromEnv(): base.GolemServer {
  const server_kind = requiredEnvVar('GOLEM_REPL_SERVER_KIND');
  switch (server_kind) {
    case 'local':
      return { type: 'local' };
    case 'cloud':
      return {
        type: 'cloud',
        token: requiredEnvVar('GOLEM_REPL_SERVER_TOKEN'),
      };
    case 'custom':
      return {
        type: 'custom',
        url: requiredEnvVar('GOLEM_REPL_SERVER_CUSTOM_URL'),
        token: requiredEnvVar('GOLEM_REPL_SERVER_TOKEN'),
      };
    default:
      throw new Error(`Invalid GOLEM_REPL_SERVER_KIND: ${server_kind}`);
  }
}

function requiredEnvVar(name: string): string {
  const value = env[name];
  if (!value) {
    throw new Error(`Missing required environment variable: ${name}`);
  }
  return value;
}

export function loadReplCliFlags(): ReplCliFlags {
  const normalizedArgs = process.argv
    .slice(2)
    .map((arg) => (arg === '-script-file' ? '--script-file' : arg));

  const { values } = util.parseArgs({
    args: normalizedArgs,
    options: {
      script: { type: 'string' },
      'script-file': { type: 'string' },
      'disable-auto-imports': { type: 'boolean' },
      'disable-type-info': { type: 'boolean' },
      'disable-stream': { type: 'boolean' },
    },
    allowPositionals: true,
  });

  const streamLogs = !(values['disable-stream'] ?? false);
  const disableAutoImports = values['disable-auto-imports'] ?? false;
  const showTypeInfo = !(values['disable-type-info'] ?? false);
  const scriptPath = values['script-file'];

  const script = (() => {
    if (values.script !== undefined) {
      return values.script;
    }
    if (scriptPath !== undefined) {
      fs.readFileSync(scriptPath, 'utf8');
    }
    return undefined;
  })();

  return {
    script,
    scriptPath,
    disableAutoImports,
    showTypeInfo,
    streamLogs,
  };
}
