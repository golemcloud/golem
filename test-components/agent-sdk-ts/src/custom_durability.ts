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

import { z } from "zod";
import {
  defineAgent,
  durableAsync,
  FunctionType,
  method,
} from "@golemcloud/golem-ts-sdk";

const generator = {
  iface: "golem-it",
  function: "test-generator",
  functionType: FunctionType.writeRemote,
  requestSchema: z.object({ payload: z.string() }),
  success: z.string(),
};

const generatorParent = {
  iface: "golem-it",
  function: "test-generator-parent",
  functionType: FunctionType.writeRemote,
  requestSchema: z.object({}),
  success: z.string(),
};

async function callback(port: number, payload: string): Promise<string> {
  const response = await fetch(
    `http://localhost:${port}/callback?payload=${encodeURIComponent(payload)}`,
  );
  return await response.text();
}

function durableCallback(port: number, payload: string): Promise<string> {
  return durableAsync(generator, { payload }, () => callback(port, payload));
}

export const CustomDurabilityTs = defineAgent({
  name: "CustomDurabilityTs",
  id: { name: z.string() },
  methods: {
    callback: method({
      input: { port: z.number(), payload: z.string() },
      returns: z.string(),
    }),
    nested: method({
      input: { port: z.number() },
      returns: z.array(z.string()),
    }),
    concurrentSiblings: method({
      input: { port: z.number() },
      returns: z.array(z.string()),
    }),
    outsideScopeWhileWaiting: method({
      input: { port: z.number() },
      returns: z.array(z.string()),
    }),
  },
});

export const CustomDurabilityTsImpl = CustomDurabilityTs.implement({
  init: () => ({}),
  methods: {
    async callback({ port, payload }) {
      return await callback(port, payload);
    },

    async nested({ port }) {
      const nested = await durableAsync(generatorParent, {}, () =>
        durableCallback(port, "nested"),
      );
      const sibling = await durableCallback(port, "root-sibling");
      return [nested, sibling];
    },

    async concurrentSiblings({ port }) {
      return await Promise.all([
        durableCallback(port, "slow"),
        durableCallback(port, "fast"),
      ]);
    },

    async outsideScopeWhileWaiting({ port }) {
      const scoped = durableCallback(port, "scoped");
      const outside = await callback(port, "outside");
      return [await scoped, outside];
    },
  },
});
