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

export type Config = {
  agents: Record<string, AgentConfig>;
  historyFile: string;
};

export type AgentConfig = {
  clientPackageName: string;
  clientPackageImportedName: string;
  package: any;
};

export type GolemServer =
  | { type: 'local' }
  | { type: 'cloud'; token: string }
  | { type: 'custom'; url: string; token: string };

export type ClientConfiguration = {
  server: GolemServer;
  application: ApplicationName;
  environment: EnvironmentName;
};

export type ApplicationName = string;
export type EnvironmentName = string;

export type ConfigureClient = (config: ClientConfiguration) => void;

export function client_configuration_from_env(): ClientConfiguration {
  return {
    server: client_server_configuration_from_env(),
    application: required_env_var('GOLEM_REPL_APPLICATION'),
    environment: required_env_var('GOLEM_REPL_ENVIRONMENT'),
  };
}

function client_server_configuration_from_env(): GolemServer {
  const server_kind = required_env_var('GOLEM_REPL_SERVER_KIND');
  switch (server_kind) {
    case 'local':
      return { type: 'local' };
    case 'cloud':
      return {
        type: 'cloud',
        token: required_env_var('GOLEM_REPL_SERVER_TOKEN'),
      };
    case 'custom':
      return {
        type: 'custom',
        url: required_env_var('GOLEM_REPL_SERVER_CUSTOM_URL'),
        token: required_env_var('GOLEM_REPL_SERVER_TOKEN'),
      };
    default:
      throw new Error(`Invalid GOLEM_REPL_SERVER_KIND: ${server_kind}`);
  }
}

function required_env_var(name: string): string {
  const value = env[name];
  if (!value) {
    throw new Error(`Missing required environment variable: ${name}`);
  }
  return value;
}
