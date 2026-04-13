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

import { BaseAgent, agent, acquireQuotaToken, QuotaToken } from '@golemcloud/golem-ts-sdk';

@agent()
class QuotaRpcReceiver extends BaseAgent {
  constructor(private readonly _name: string) {
    super();
  }

  async reserveAndCallInLoop(
    childToken: QuotaToken,
    host: string,
    port: number,
    maxIterations: number,
  ): Promise<void> {
    for (let i = 0; i < maxIterations; i++) {
      const result = childToken.reserve(1n);
      if (result.isErr()) break;

      const reservation = result.unwrap();
      await fetch(`http://${host}:${port}/call`);
      reservation.commit(1n);
    }
  }
}

@agent()
class QuotaRpcSender extends BaseAgent {
  constructor(private readonly name: string) {
    super();
  }

  async splitAndLoop(
    resourceName: string,
    totalExpectedUse: bigint,
    childExpectedUse: bigint,
    host: string,
    port: number,
    maxIterations: number,
  ): Promise<void> {
    const token = acquireQuotaToken(resourceName, totalExpectedUse);
    const childToken = token.split(childExpectedUse);

    const receiverName = `${this.name}-receiver`;
    QuotaRpcReceiver.get(receiverName).reserveAndCallInLoop.trigger(
      childToken,
      host,
      port,
      maxIterations,
    );

    for (let i = 0; i < maxIterations; i++) {
      const result = token.reserve(1n);
      if (result.isErr()) break;

      const reservation = result.unwrap();
      await fetch(`http://${host}:${port}/call`);
      reservation.commit(1n);
    }
  }
}
