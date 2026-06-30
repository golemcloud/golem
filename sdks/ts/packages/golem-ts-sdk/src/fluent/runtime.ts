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

// Self-contained fluent runtime, built ONLY on the new schema model: it compiles
// id/method schemas to `FluentCodec`s, assembles the WIT `AgentType` via
// `GraphEncoder`, and dispatches decode → handler (`this` = state) → encode.
// Deliberately avoids the decorator-era machinery (`Type.Type`, `ResolvedAgent`,
// `boundaryValue.ts`) so it stands alone once the decorator SDK is removed.

import {
  AgentConstructor,
  AgentError,
  AgentMethod,
  AgentType,
  InputSchema,
  OutputSchema,
  Principal as HostPrincipal,
} from 'golem:agent/common@2.0.0';
import { Result } from 'golem:agent/host@2.0.0';
import { SchemaValueTree } from 'golem:core/types@2.0.0';
import {
  emptyMetadata,
  GraphEncoder,
  mergeGraphDefs,
  SchemaGraph,
  SchemaValue,
  schemaValueFromWit,
  schemaValueToWit,
} from '../internal/schema-model';
import { AgentClassName } from '../agentClassName';
import { AgentTypeRegistry } from '../internal/registry/agentTypeRegistry';
import { AgentInitiatorRegistry } from '../internal/registry/agentInitiatorRegistry';
import { getRawSelfAgentId } from '../host/hostapi';
import { createCustomError, invalidInput, invalidMethod } from '../internal/agentError';
import { sdkPrincipalFromHost } from '../principal';
import { ParsedAgentId } from '../agentId';
import {
  DatabaseSync,
  Session,
  SQLTagStore,
  StatementSync,
  isAutocommitDatabaseSync,
  restoreDatabaseSync,
  serializeDatabaseSync,
} from '../internal/sqlite';
import { decodeMultipart, encodeMultipart, MultipartPart } from '../internal/multipart';
import { compileSchema } from './schema/adapter';
import { FluentCodec } from './schema/codec';
import type { AgentImplementation, IdRecord, MethodsRecord } from './defineAgent';

/** A named parameter and its compiled codec, in declaration order. */
interface NamedCodec {
  name: string;
  codec: FluentCodec;
}

/** A compiled method: ordered input codecs + a unit-or-single output. */
interface MethodCodec {
  name: string;
  inputCodecs: NamedCodec[];
  output: { tag: 'unit' } | { tag: 'single'; codec: FluentCodec };
}

/** Compiled agent: the assembled `AgentType` plus the per-schema codecs. */
export interface RegisteredAgent {
  name: string;
  className: AgentClassName;
  agentType: AgentType;
  idCodecs: NamedCodec[];
  methodCodecs: Map<string, MethodCodec>;
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Compile id + method schemas to codecs and assemble + register the `AgentType`. */
export function registerAgentType(
  name: string,
  id: IdRecord,
  methods: MethodsRecord,
): RegisteredAgent {
  const className = new AgentClassName(name);

  // Declaration order (Object.keys) is the single authoritative field order; it
  // drives both the AgentType named-field list and the value record codec.
  const idCodecs: NamedCodec[] = Object.keys(id).map((k) => ({ name: k, codec: compileSchema(id[k]) }));

  const methodCodecs = new Map<string, MethodCodec>();
  for (const [methodName, spec] of Object.entries(methods)) {
    const inputCodecs: NamedCodec[] = Object.keys(spec.input).map((k) => ({
      name: k,
      codec: compileSchema(spec.input[k]),
    }));
    const returnsCodec = compileSchema(spec.returns);
    const output: MethodCodec['output'] = returnsCodec.isUnit
      ? { tag: 'unit' }
      : { tag: 'single', codec: returnsCodec };
    methodCodecs.set(methodName, { name: methodName, inputCodecs, output });
  }

  const agentType = assembleAgentType(name, idCodecs, methodCodecs);
  AgentTypeRegistry.register(className, agentType);

  return { name, className, agentType, idCodecs, methodCodecs };
}

/**
 * Build the WIT `AgentType` from the compiled codecs: merge the per-schema graphs
 * into one pool and encode each root into a shared `schema-graph` via
 * `GraphEncoder`. The decorator-SDK analog is `buildAgentType`.
 */
function assembleAgentType(
  name: string,
  idCodecs: NamedCodec[],
  methodCodecs: Map<string, MethodCodec>,
): AgentType {
  const graphs: SchemaGraph[] = [];
  for (const ic of idCodecs) graphs.push(ic.codec.graph);
  for (const mc of methodCodecs.values()) {
    for (const ic of mc.inputCodecs) graphs.push(ic.codec.graph);
    if (mc.output.tag === 'single') graphs.push(mc.output.codec.graph);
  }

  const encoder = new GraphEncoder(mergeGraphDefs(graphs));

  const encodeInput = (codecs: NamedCodec[]): InputSchema => ({
    tag: 'parameters',
    val: codecs.map((c) => ({
      name: c.name,
      source: { tag: 'user-supplied' },
      schema: encoder.encodeType(c.codec.graph.root),
      metadata: emptyMetadata(),
    })),
  });

  const constructorInput = encodeInput(idCodecs);

  const methods: AgentMethod[] = [];
  for (const mc of methodCodecs.values()) {
    const outputSchema: OutputSchema =
      mc.output.tag === 'unit'
        ? { tag: 'unit' }
        : { tag: 'single', val: encoder.encodeType(mc.output.codec.graph.root) };
    methods.push({
      name: mc.name,
      description: '',
      promptHint: undefined,
      httpEndpoint: [],
      readOnly: undefined,
      inputSchema: encodeInput(mc.inputCodecs),
      outputSchema,
    });
  }

  const description = `Constructs the agent ${name}`;
  const constructor: AgentConstructor = {
    name: undefined,
    description,
    promptHint: idCodecs.length
      ? `Enter the following parameters: ${idCodecs.map((c) => c.name).join(', ')}`
      : undefined,
    inputSchema: constructorInput,
  };

  return {
    typeName: name,
    description,
    sourceLanguage: 'typescript',
    schema: encoder.finish(),
    constructor,
    methods,
    dependencies: [],
    mode: 'durable',
    httpMount: undefined,
    snapshotting: { tag: 'disabled' },
    config: [],
  };
}

/**
 * Self-contained resolved agent (the decorator `ResolvedAgent` analog) exposing
 * exactly what the guest entry calls: `invoke` / `getAgentType` / `getId` /
 * `saveSnapshot` / `loadSnapshot`.
 */
class FluentResolvedAgent {
  constructor(
    private readonly reg: RegisteredAgent,
    /** The handler `this`: state fields + `getId`/`getPhantomId` helpers. */
    private readonly instance: Record<string, unknown>,
    private readonly methods: Record<string, (...args: unknown[]) => unknown>,
    private readonly agentId: ParsedAgentId,
  ) {}

  getAgentType(): AgentType {
    return this.reg.agentType;
  }

  getId(): ParsedAgentId {
    return this.agentId;
  }

  async invoke(
    methodName: string,
    methodArgs: SchemaValueTree,
    _principal: HostPrincipal,
  ): Promise<Result<SchemaValueTree | undefined, AgentError>> {
    const mc = this.reg.methodCodecs.get(methodName);
    if (!mc) {
      return { tag: 'err', val: invalidMethod(`Method ${methodName} not found on agent ${this.reg.name}`) };
    }
    const handler = this.methods[methodName];
    if (typeof handler !== 'function') {
      return {
        tag: 'err',
        val: invalidMethod(`No handler for method ${methodName} on agent ${this.reg.name}`),
      };
    }

    let args: unknown;
    try {
      if (mc.inputCodecs.length === 0) {
        args = undefined;
      } else {
        const inputValue = schemaValueFromWit(methodArgs);
        const fields = (inputValue as Extract<SchemaValue, { tag: 'record' }>).fields;
        const record: Record<string, unknown> = {};
        mc.inputCodecs.forEach((ic, i) => {
          record[ic.name] = ic.codec.fromValue(fields[i]);
        });
        args = record;
      }
    } catch (e) {
      return {
        tag: 'err',
        val: invalidInput(
          `Failed to decode input for ${methodName} on agent ${this.reg.name}: ${errorMessage(e)}`,
        ),
      };
    }

    let result: unknown;
    try {
      result =
        mc.inputCodecs.length === 0
          ? await handler.call(this.instance)
          : await handler.call(this.instance, args);
    } catch (e) {
      return { tag: 'err', val: createCustomError(errorMessage(e)) };
    }

    try {
      if (mc.output.tag === 'unit') {
        return { tag: 'ok', val: undefined };
      }
      return { tag: 'ok', val: schemaValueToWit(mc.output.codec.toValue(result)) };
    } catch (e) {
      return {
        tag: 'err',
        val: createCustomError(`Failed to encode result of ${methodName}: ${errorMessage(e)}`),
      };
    }
  }

  // Structural snapshot: JSON of plain state fields, plus a `db:<field>` part per
  // `DatabaseSync` field — reusing the SDK's existing SQLite/multipart helpers.
  // The principal/version envelope is added by the guest (`src/index.ts`).
  async saveSnapshot(): Promise<{ data: Uint8Array; mimeType: string }> {
    const state: Record<string, unknown> = {};
    const databases: Array<{ name: string; bytes: Uint8Array }> = [];
    const seen = new Set<unknown>();

    for (const [k, val] of Object.entries(this.instance)) {
      if (typeof val === 'function') continue;
      if (val instanceof DatabaseSync) {
        if (seen.has(val)) {
          throw `Multiple agent fields reference the same DatabaseSync instance (field "${k}").`;
        }
        seen.add(val);
        if (!isAutocommitDatabaseSync(val)) {
          throw `Cannot snapshot database "${k}": an open transaction exists. Commit or rollback before saving.`;
        }
        databases.push({ name: k, bytes: serializeDatabaseSync(val) });
        continue;
      }
      if (val instanceof StatementSync || val instanceof Session || val instanceof SQLTagStore) {
        continue;
      }
      state[k] = val;
    }

    if (databases.length === 0) {
      return { data: new TextEncoder().encode(JSON.stringify(state)), mimeType: 'application/json' };
    }

    const parts: MultipartPart[] = [
      {
        name: 'state',
        contentType: 'application/json',
        body: new TextEncoder().encode(JSON.stringify(state)),
      },
      ...databases.map((db) => ({
        name: `db:${db.name}`,
        contentType: 'application/x-sqlite3',
        body: db.bytes,
      })),
    ];
    const { data, boundary } = encodeMultipart(parts);
    return { data, mimeType: `multipart/mixed; boundary=${boundary}` };
  }

  async loadSnapshot(bytes: Uint8Array, mimeType?: string): Promise<void> {
    if (mimeType && mimeType.startsWith('multipart/mixed')) {
      const boundary = mimeType.match(/boundary=([^\s;]+)/)?.[1];
      if (!boundary) throw 'multipart/mixed snapshot missing boundary parameter';
      const parts = decodeMultipart(bytes, boundary);
      const statePart = parts.find((p) => p.name === 'state');
      if (!statePart) throw 'multipart snapshot missing "state" part';
      Object.assign(this.instance, JSON.parse(new TextDecoder().decode(statePart.body)));
      for (const p of parts) {
        if (!p.name.startsWith('db:')) continue;
        const field = this.instance[p.name.slice(3)];
        if (field instanceof DatabaseSync) restoreDatabaseSync(field, p.body);
      }
      return;
    }
    Object.assign(this.instance, JSON.parse(new TextDecoder().decode(bytes)));
  }
}

/** Register the agent's initiator. On `initiate`, decode id, run `init`, wire handlers. */
export function registerAgentInitiator(
  reg: RegisteredAgent,
  impl: AgentImplementation<IdRecord, MethodsRecord, object>,
): void {
  AgentInitiatorRegistry.register(reg.className, {
    async initiate(constructorInput: SchemaValue, principal: HostPrincipal) {
      let idRecord: Record<string, unknown>;
      try {
        const fields = (constructorInput as Extract<SchemaValue, { tag: 'record' }>).fields;
        idRecord = {};
        reg.idCodecs.forEach((ic, i) => {
          idRecord[ic.name] = ic.codec.fromValue(fields[i]);
        });
      } catch (e) {
        return {
          tag: 'err',
          val: createCustomError(
            `Failed to deserialize constructor arguments for agent ${reg.name}: ${errorMessage(e)}`,
          ),
        };
      }

      const agentId = getRawSelfAgentId();
      if (!agentId.value.startsWith(reg.name)) {
        return {
          tag: 'err',
          val: createCustomError(
            `Expected the container name to start with "${reg.name}", got "${agentId.value}"`,
          ),
        };
      }
      const [, , phantomId] = agentId.parsed();
      const sdkPrincipal = sdkPrincipalFromHost(principal);

      // `init` may be synchronous or async (return a Promise); awaiting a plain
      // value is a no-op, so both forms work. The guest `initialize`/load-snapshot
      // paths await the initiate result.
      let state: object;
      try {
        state = await impl.init({ id: idRecord as never, principal: sdkPrincipal, phantomId });
      } catch (e) {
        return {
          tag: 'err',
          val: createCustomError(`Agent ${reg.name} initialization failed: ${errorMessage(e)}`),
        };
      }

      const instance: Record<string, unknown> = { ...(state as Record<string, unknown>) };
      instance.getId = () => agentId;
      instance.getPhantomId = () => phantomId;
      instance.getPrincipal = () => sdkPrincipal;

      return {
        tag: 'ok',
        val: new FluentResolvedAgent(
          reg,
          instance,
          impl.methods as Record<string, (...args: unknown[]) => unknown>,
          agentId,
        ) as never,
      };
    },
  });
}
