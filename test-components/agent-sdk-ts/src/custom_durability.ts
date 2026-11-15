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

// The body sleeps on a timer before its callback, so the timer's sleep belongs to the operation.
function timedCallback(port: number, payload: string): Promise<string> {
  return durableAsync(generator, { payload }, async () => {
    await new Promise<void>((resolve) => setTimeout(resolve, 1));
    return await callback(port, payload);
  });
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
    unawaitedChild: method({
      input: { port: z.number() },
      returns: z.string(),
    }),
    failingBody: method({
      input: { port: z.number() },
      returns: z.string(),
    }),
    escapedWork: method({
      input: { port: z.number() },
      returns: z.array(z.string()),
    }),
    concurrentFirstFinishes: method({
      input: { port: z.number() },
      returns: z.array(z.string()),
    }),
    cancelledCall: method({
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

    async unawaitedChild({ port }) {
      return await durableAsync(generatorParent, {}, async () => {
        void durableCallback(port, "detached-child");
        return "parent";
      });
    },

    async failingBody({ port }) {
      try {
        await durableAsync(generator, { payload: "failing" }, async () => {
          await callback(port, "failing");
          throw new Error("body failed");
        });
        return "finished";
      } catch (error) {
        await callback(port, "after-failure");
        return (error as Error).message;
      }
    },

    async escapedWork({ port }) {
      let release!: () => void;
      const gate = new Promise<void>((resolve) => (release = resolve));
      let escaped!: Promise<string>;
      const scoped = await durableAsync(
        generator,
        { payload: "escape-scoped" },
        async () => {
          escaped = gate.then(() => callback(port, "escaped"));
          return await callback(port, "escape-scoped");
        },
      );
      release();
      return [scoped, await escaped];
    },

    async concurrentFirstFinishes({ port }) {
      return await Promise.all([
        (async () => {
          const first = await timedCallback(port, "first");
          await (await fetch(`http://localhost:${port}/release`)).text();
          return first;
        })(),
        durableCallback(port, "second"),
      ]);
    },

    // The body aborts a request that the server holds, after the server confirmed its arrival.
    async cancelledCall({ port }) {
      const scoped = await durableAsync(
        generator,
        { payload: "cancel-scoped" },
        async () => {
          const controller = new AbortController();
          const hanging = fetch(
            `http://localhost:${port}/callback?payload=hang`,
            { signal: controller.signal },
          );
          await callback(port, "hang-ready");
          controller.abort();
          const outcome = await hanging.then(
            () => "not-aborted",
            (error: unknown) => `aborted-${(error as Error).name}`,
          );
          await callback(port, "after-abort");
          return outcome;
        },
      );
      return [scoped, await callback(port, "after-cancel")];
    },
  },
});
