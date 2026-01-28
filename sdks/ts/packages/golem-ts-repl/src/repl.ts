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
  client_configuration_from_env,
  Config,
  ConfigureClient,
} from './config';
import * as repl from 'node:repl';

export class Repl {
  private readonly config: Config;

  constructor(config: Config) {
    this.config = config;
  }

  async run() {
    const client_config = client_configuration_from_env();

    const r = repl.start({
      useColors: true,
      useGlobal: true,
    });

    for (let agentTypeName in this.config.agents) {
      const agentConfig = this.config.agents[agentTypeName];
      let configure = agentConfig.package.configure as ConfigureClient;
      configure(client_config);
      r.context[agentConfig.typeName] =
        agentConfig.package[agentConfig.typeName];
    }
  }
}
