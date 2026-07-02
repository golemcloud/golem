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

// Fluent wasm-RPC client. `clientFor(def)(id)` returns a typed proxy that calls a
// remote agent declared with the same `defineAgent` definition. The wire encoding
// is built from the LOCAL def's `FluentCodec`s — the exact codecs the exported
// component uses to decode (see runtime.ts `invoke`) — so the two sides are
// symmetric by construction. Reuses the host `WasmRpc` resource (no decorator
// `Type.Type`/metadata).

import { makeAgentId, WasmRpc, Datetime } from 'golem:agent/host@2.0.0';
import { schemaValueToWit, schemaValueFromWit, v } from '../internal/schema-model';
import { awaitPollable } from '../internal/pollableUtils';
import { Uuid } from '../uuid';
import { compileSchema } from './schema/adapter';
import { FluentCodec } from './schema/codec';
import { StandardSchemaV1 } from './schema/standardSchema';
import { AgentDefinition, IdRecord, MethodsRecord } from './defineAgent';
import { MethodSpec } from './method';

type InferRecord<R extends Record<string, StandardSchemaV1>> = {
  [K in keyof R]: StandardSchemaV1.InferOutput<R[K]>;
};

/** The async remote signature for a method spec (no-arg when input is empty). */
type RemoteMethodFor<M> =
  M extends MethodSpec<infer Input, infer Output>
    ? keyof Input extends never
      ? {
          (): Promise<Output>;
          /** Fire-and-forget; no result is awaited. */
          trigger(): void;
          /** Enqueue the call to run at `at`. */
          schedule(at: Datetime): void;
        }
      : {
          (input: InferRecord<Input>): Promise<Output>;
          trigger(input: InferRecord<Input>): void;
          schedule(at: Datetime, input: InferRecord<Input>): void;
        }
    : never;

/** A typed remote client: one async method per declared method on the def. */
export type RemoteClient<Methods extends MethodsRecord> = {
  [K in keyof Methods]: RemoteMethodFor<Methods[K]>;
};

interface NamedCodec {
  name: string;
  codec: FluentCodec;
}
interface CompiledRemoteMethod {
  name: string;
  inputCodecs: NamedCodec[];
  output: { tag: 'unit' } | { tag: 'single'; codec: FluentCodec };
}

/** Raised when a remote agent invocation fails or returns an error result. */
export class RemoteCallError extends Error {
  readonly _tag = 'RemoteCallError';
}

/** Encode a method/constructor input record (positional, declaration order). */
function encodeRecord(codecs: NamedCodec[], input: Record<string, unknown>) {
  return schemaValueToWit(v.record(codecs.map((c) => c.codec.toValue(input[c.name]))));
}

/**
 * Build a typed RPC client factory for a remote agent definition.
 *
 * ```ts
 * const counter = clientFor(CounterDef);
 * const c1 = counter({ name: 'c1' });
 * const next = await c1.increment({ by: 5 });
 * c1.increment.trigger({ by: 1 }); // fire-and-forget
 * ```
 */
export function clientFor<Id extends IdRecord, Methods extends MethodsRecord>(
  def: AgentDefinition<Id, Methods>,
): (id: InferRecord<Id>, phantomId?: Uuid) => RemoteClient<Methods> {
  // Compile the def's id + method codecs once (cached in this closure).
  const idCodecs: NamedCodec[] = Object.keys(def.id).map((k) => ({ name: k, codec: compileSchema(def.id[k]) }));
  const methodCodecs: CompiledRemoteMethod[] = Object.entries(def.methods).map(([name, spec]) => {
    const inputCodecs: NamedCodec[] = Object.keys((spec as MethodSpec).input).map((k) => ({
      name: k,
      codec: compileSchema((spec as MethodSpec).input[k]),
    }));
    const retCodec = compileSchema((spec as MethodSpec).returns);
    const output = retCodec.isUnit
      ? ({ tag: 'unit' } as const)
      : ({ tag: 'single', codec: retCodec } as const);
    return { name, inputCodecs, output };
  });

  return (id: InferRecord<Id>, phantomId?: Uuid): RemoteClient<Methods> => {
    const constructorTree = encodeRecord(idCodecs, id as Record<string, unknown>);
    const agentId = makeAgentId(def.name, constructorTree, phantomId);
    const wasmRpc = new WasmRpc(def.name, constructorTree, phantomId, []);

    const decodeOutput = (mc: CompiledRemoteMethod, val: unknown): unknown => {
      if (mc.output.tag === 'unit' || val === undefined) return undefined;
      return mc.output.codec.fromValue(schemaValueFromWit(val as Parameters<typeof schemaValueFromWit>[0]));
    };

    const client: Record<string, unknown> = {};
    for (const mc of methodCodecs) {
      const methodFn = async (input: Record<string, unknown> = {}) => {
        const inputTree = encodeRecord(mc.inputCodecs, input);
        const future = wasmRpc.asyncInvokeAndAwait(mc.name, inputTree);
        await awaitPollable(future.subscribe());
        const result = future.get();
        if (!result) {
          throw new RemoteCallError(`RPC to ${agentId}.${mc.name} failed (no result)`);
        }
        if (result.tag === 'err') {
          throw new RemoteCallError(`Remote agent ${agentId}.${mc.name} errored: ${JSON.stringify(result.val)}`);
        }
        return decodeOutput(mc, result.val);
      };
      methodFn.trigger = (input: Record<string, unknown> = {}) => {
        wasmRpc.invoke(mc.name, encodeRecord(mc.inputCodecs, input));
      };
      methodFn.schedule = (at: Datetime, input: Record<string, unknown> = {}) => {
        wasmRpc.scheduleInvocation(at, mc.name, encodeRecord(mc.inputCodecs, input));
      };
      client[mc.name] = methodFn;
    }
    return client as RemoteClient<Methods>;
  };
}
