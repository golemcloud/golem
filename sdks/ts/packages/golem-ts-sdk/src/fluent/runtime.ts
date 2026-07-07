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
  AgentDependency,
  AgentError,
  AgentMethod,
  AgentType,
  InputSchema,
  OutputSchema,
  Principal as HostPrincipal,
  Snapshotting,
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
import { StandardSchemaV1 } from './schema/standardSchema';
import type {
  AgentImplementation,
  AgentMetadataSpec,
  IdRecord,
  MethodsRecord,
  SnapshotPolicy,
  SnapshottingSpec,
} from './defineAgent';
import { MethodSpec, ReadOnlyOption } from './method';
import { buildConfigAccessor, compileConfig, ConfigDeclaration, ConfigSpec } from './config';
import { compileEndpoint, compileMount, pathVariableNames } from './http';
import {
  HttpEndpointDetails,
  HttpMountDetails,
  ReadOnlyConfig,
  CachePolicy,
} from 'golem:agent/common@2.0.0';

/**
 * Resolve a method's `readOnly` option to the WIT `read-only-config`, or
 * `undefined` when the method is not read-only. A bare `true` uses the
 * `until-write` cache policy (matching the base SDK default); an object form
 * selects `no-cache` / `until-write` / `ttl` and per-principal caching.
 */
function resolveReadOnly(
  readOnly: boolean | ReadOnlyOption | undefined,
): ReadOnlyConfig | undefined {
  if (!readOnly) return undefined;
  const opt: ReadOnlyOption = readOnly === true ? {} : readOnly;
  const cache = opt.cache;
  let cachePolicy: CachePolicy;
  if (cache === undefined || cache === 'until-write') {
    cachePolicy = { tag: 'until-write' };
  } else if (cache === 'no-cache') {
    cachePolicy = { tag: 'no-cache' };
  } else {
    cachePolicy = { tag: 'ttl', val: cache.ttlNanos };
  }
  return { cachePolicy, usesPrincipal: opt.usesPrincipal ?? false };
}

/** A named parameter and its compiled codec, in declaration order. */
interface NamedCodec {
  name: string;
  codec: FluentCodec;
}

/** A compiled method: ordered input codecs + a unit-or-single output + metadata. */
interface MethodCodec {
  name: string;
  inputCodecs: NamedCodec[];
  output: { tag: 'unit' } | { tag: 'single'; codec: FluentCodec };
  /** Method-level metadata (description / promptHint / readOnly). */
  meta: Pick<MethodSpec, 'description' | 'promptHint' | 'readOnly'>;
  /** Compiled WIT HTTP endpoints declared on this method (empty if none). */
  httpEndpoints: HttpEndpointDetails[];
}

/** Compiled agent: the assembled `AgentType` plus the per-schema codecs. */
export interface RegisteredAgent {
  name: string;
  className: AgentClassName;
  agentType: AgentType;
  idCodecs: NamedCodec[];
  methodCodecs: Map<string, MethodCodec>;
  configDeclarations: ConfigDeclaration[];
  /**
   * Typed snapshot-state schema from `snapshotting: { state }`. When set, the
   * JSON snapshot is scoped to + validated by this schema (only the declared
   * state fields of `this` are persisted); otherwise `this` is snapshotted by
   * reflection.
   */
  snapshotStateSchema?: StandardSchemaV1;
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Compile id + method schemas to codecs and assemble + register the `AgentType`. */
export function registerAgentType(
  name: string,
  id: IdRecord,
  methods: MethodsRecord,
  metadata: AgentMetadataSpec = {},
): RegisteredAgent {
  const className = new AgentClassName(name);

  // Declaration order (Object.keys) is the single authoritative field order; it
  // drives both the AgentType named-field list and the value record codec.
  const idCodecs: NamedCodec[] = Object.keys(id).map((k) => ({
    name: k,
    codec: compileSchema(id[k]),
  }));

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

    const httpSpecs =
      spec.http === undefined ? [] : Array.isArray(spec.http) ? spec.http : [spec.http];
    const httpEndpoints = httpSpecs.map((ep) => {
      try {
        return compileEndpoint(ep);
      } catch (e) {
        throw new Error(
          `Agent "${name}" method "${methodName}" has an invalid HTTP endpoint: ${errorMessage(e)}`,
        );
      }
    });

    methodCodecs.set(methodName, {
      name: methodName,
      inputCodecs,
      output,
      meta: {
        description: spec.description,
        promptHint: spec.promptHint,
        readOnly: spec.readOnly,
      },
      httpEndpoints,
    });
  }

  const configDeclarations = compileConfig(metadata.config);

  const agentType = assembleAgentType(name, idCodecs, methodCodecs, configDeclarations, metadata);
  AgentTypeRegistry.register(className, agentType);

  const snap = metadata.snapshotting;
  const snapshotStateSchema =
    snap !== undefined && typeof snap === 'object' && 'state' in snap ? snap.state : undefined;
  return {
    name,
    className,
    agentType,
    idCodecs,
    methodCodecs,
    configDeclarations,
    snapshotStateSchema,
  };
}

/**
 * Build the WIT `AgentType` from the compiled codecs: merge the per-schema graphs
 * into one pool and encode each root into a shared `schema-graph` via
 * `GraphEncoder`. The decorator-SDK analog is `buildAgentType`.
 */
/** Extract the WHEN-policy from a snapshotting spec (the `{ policy, state }` form defaults to `'default'`). */
function snapshotPolicyOf(spec: SnapshottingSpec | undefined): SnapshotPolicy | undefined {
  if (spec !== undefined && typeof spec === 'object' && 'state' in spec)
    return spec.policy ?? 'default';
  return spec;
}

/** Map the fluent {@link SnapshottingSpec} to the WIT `snapshotting` variant. */
function toWitSnapshotting(spec: SnapshottingSpec | undefined): Snapshotting {
  const policy = snapshotPolicyOf(spec);
  if (policy === undefined || policy === 'disabled') return { tag: 'disabled' };
  if (policy === 'default') return { tag: 'enabled', val: { tag: 'default' } };
  if ('periodicSeconds' in policy) {
    // WIT `periodic` takes a `duration` (u64 nanoseconds).
    const seconds = policy.periodicSeconds < 0 ? 0 : policy.periodicSeconds;
    return { tag: 'enabled', val: { tag: 'periodic', val: BigInt(Math.round(seconds * 1e9)) } };
  }
  return { tag: 'enabled', val: { tag: 'every-n-invocation', val: policy.everyNInvocations } };
}

/**
 * Resolve the declared dependency type-names into WIT `agent-dependency`
 * records, reusing each dependency's already-registered `AgentType`. Throws a
 * clear error if a dependency has not been registered yet.
 */
function resolveDependencies(name: string, depNames: readonly string[]): AgentDependency[] {
  return depNames.map((depName) => {
    const dep = AgentTypeRegistry.get(new AgentClassName(depName));
    if (dep === undefined) {
      throw new Error(
        `Agent "${name}" declares a dependency on "${depName}", but "${depName}" has not been ` +
          `registered yet. Define the dependency agent before the agent that depends on it.`,
      );
    }
    return {
      typeName: dep.typeName,
      description: dep.description,
      schema: dep.schema,
      constructor: dep.constructor,
      methods: dep.methods,
    };
  });
}

/** Compile the agent's HTTP mount, wrapping parse failures with the agent name. */
function compileHttpMount(
  name: string,
  http: NonNullable<AgentMetadataSpec['http']>,
): HttpMountDetails {
  try {
    return compileMount(http);
  } catch (e) {
    throw new Error(`Agent "${name}" has an invalid HTTP mount: ${errorMessage(e)}`);
  }
}

/**
 * Registry-free consistency checks for the fluent HTTP surface:
 *  - a method declaring endpoints requires the agent to have a mount;
 *  - every `{var}` in the mount prefix must be an id-record field;
 *  - every path/query/header variable in an endpoint must be a method input
 *    parameter (mount path vars are also accepted, since the host resolves them
 *    from the constructor-supplied id).
 */
function validateHttpConsistency(
  name: string,
  httpMount: HttpMountDetails | undefined,
  idCodecs: NamedCodec[],
  methodCodecs: Map<string, MethodCodec>,
): void {
  const idNames = new Set(idCodecs.map((c) => c.name));
  const mountVars = httpMount ? pathVariableNames(httpMount.pathPrefix) : new Set<string>();

  if (httpMount) {
    for (const v of mountVars) {
      if (!idNames.has(v)) {
        throw new Error(
          `Agent "${name}" HTTP mount references path variable "${v}", but it is not a field of ` +
            `the agent id. Mount path variables must match id fields.`,
        );
      }
    }
  }

  for (const mc of methodCodecs.values()) {
    if (mc.httpEndpoints.length === 0) continue;
    if (!httpMount) {
      throw new Error(
        `Agent "${name}" method "${mc.name}" declares HTTP endpoint(s) but the agent has no HTTP ` +
          `mount. Add an "http" mount to defineAgent.`,
      );
    }
    const inputNames = new Set(mc.inputCodecs.map((c) => c.name));
    for (const ep of mc.httpEndpoints) {
      for (const v of pathVariableNames(ep.pathSuffix)) {
        assertEndpointVar(name, mc.name, v, 'path', inputNames, mountVars);
      }
      for (const q of ep.queryVars) {
        assertEndpointVar(name, mc.name, q.variableName, 'query', inputNames, mountVars);
      }
      for (const h of ep.headerVars) {
        assertEndpointVar(name, mc.name, h.variableName, 'header', inputNames, mountVars);
      }
    }
  }
}

function assertEndpointVar(
  name: string,
  methodName: string,
  variable: string,
  location: 'path' | 'query' | 'header',
  inputNames: Set<string>,
  mountVars: Set<string>,
): void {
  if (inputNames.has(variable) || mountVars.has(variable)) return;
  throw new Error(
    `Agent "${name}" method "${methodName}" HTTP ${location} variable "${variable}" is not a ` +
      `parameter of the method (nor a mount path variable).`,
  );
}

function assembleAgentType(
  name: string,
  idCodecs: NamedCodec[],
  methodCodecs: Map<string, MethodCodec>,
  configDeclarations: ConfigDeclaration[],
  metadata: AgentMetadataSpec,
): AgentType {
  const graphs: SchemaGraph[] = [];
  for (const ic of idCodecs) graphs.push(ic.codec.graph);
  for (const mc of methodCodecs.values()) {
    for (const ic of mc.inputCodecs) graphs.push(ic.codec.graph);
    if (mc.output.tag === 'single') graphs.push(mc.output.codec.graph);
  }
  // Pool each config field's *declaration* graph (inner for local,
  // `secret<inner>` for secret) so the shared GraphEncoder includes it.
  for (const d of configDeclarations) graphs.push(d.graph);

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

  // Compile the HTTP mount (if any), then validate mount + endpoint variable
  // consistency against the id record / method inputs (registry-free checks;
  // the decorator-era validators are param-registry coupled and unusable here).
  const httpMount: HttpMountDetails | undefined = metadata.http
    ? compileHttpMount(name, metadata.http)
    : undefined;
  validateHttpConsistency(name, httpMount, idCodecs, methodCodecs);

  const methods: AgentMethod[] = [];
  for (const mc of methodCodecs.values()) {
    const outputSchema: OutputSchema =
      mc.output.tag === 'unit'
        ? { tag: 'unit' }
        : { tag: 'single', val: encoder.encodeType(mc.output.codec.graph.root) };
    methods.push({
      name: mc.name,
      description: mc.meta.description ?? '',
      promptHint: mc.meta.promptHint,
      httpEndpoint: mc.httpEndpoints,
      // `readOnly: true` → `until-write` caching (base default); the object form
      // selects no-cache / ttl / per-principal; omitted/`false` → unset.
      readOnly: resolveReadOnly(mc.meta.readOnly),
      inputSchema: encodeInput(mc.inputCodecs),
      outputSchema,
    });
  }

  // `agent-type.description` carries the agent's own description; the
  // constructor keeps its generated "Constructs the agent ..." description.
  const ctorDescription = `Constructs the agent ${name}`;
  const constructor: AgentConstructor = {
    name: undefined,
    description: ctorDescription,
    promptHint:
      metadata.promptHint ??
      (idCodecs.length
        ? `Enter the following parameters: ${idCodecs.map((c) => c.name).join(', ')}`
        : undefined),
    inputSchema: constructorInput,
  };

  return {
    typeName: name,
    description: metadata.description ?? ctorDescription,
    sourceLanguage: 'typescript',
    schema: encoder.finish(),
    constructor,
    methods,
    dependencies: resolveDependencies(name, metadata.dependencies ?? []),
    mode: metadata.mode ?? 'durable',
    httpMount,
    snapshotting: toWitSnapshotting(metadata.snapshotting),
    config: configDeclarations.map((d) => ({
      source: d.source,
      path: d.path,
      valueType: encoder.encodeType(d.graph.root),
    })),
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
    /** Optional user-supplied snapshot serializer (`implement({ snapshot })`). */
    private readonly customSnapshot?: {
      save: () => Uint8Array | Promise<Uint8Array>;
      load: (bytes: Uint8Array) => void | Promise<void>;
    },
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
      return {
        tag: 'err',
        val: invalidMethod(`Method ${methodName} not found on agent ${this.reg.name}`),
      };
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

  // Snapshot serialization. Three modes:
  //  - custom (`implement({ snapshot })`): user save/load own the bytes verbatim.
  //  - typed  (`snapshotting: { state }`): JSON of ONLY the schema-validated state
  //           fields of `this`, plus a `db:<field>` SQLite part per DatabaseSync.
  //  - reflective (default): JSON of every plain field of `this` (skipping the
  //           live `config` accessor + helpers), plus the same `db:` parts.
  // The principal/version envelope is added by the guest (`src/index.ts`).
  async saveSnapshot(): Promise<{ data: Uint8Array; mimeType: string }> {
    if (this.customSnapshot) {
      const data = await this.customSnapshot.save.call(this.instance);
      return { data, mimeType: 'application/octet-stream' };
    }

    // Attached SQLite databases (reflective in both remaining modes).
    const databases: Array<{ name: string; bytes: Uint8Array }> = [];
    const seen = new Set<unknown>();
    for (const [k, val] of Object.entries(this.instance)) {
      if (!isInstance(val, DatabaseSync)) continue;
      if (seen.has(val)) {
        throw `Multiple agent fields reference the same DatabaseSync instance (field "${k}").`;
      }
      seen.add(val);
      if (!isAutocommitDatabaseSync(val)) {
        throw `Cannot snapshot database "${k}": an open transaction exists. Commit or rollback before saving.`;
      }
      databases.push({ name: k, bytes: serializeDatabaseSync(val) });
    }

    const state = this.reg.snapshotStateSchema
      ? await validateSnapshotState(this.reg.snapshotStateSchema, this.instance)
      : this.reflectiveState();

    const stateJson = new TextEncoder().encode(JSON.stringify(state));
    if (databases.length === 0) {
      return { data: stateJson, mimeType: 'application/json' };
    }
    const parts: MultipartPart[] = [
      { name: 'state', contentType: 'application/json', body: stateJson },
      ...databases.map((db) => ({
        name: `db:${db.name}`,
        contentType: 'application/x-sqlite3',
        body: db.bytes,
      })),
    ];
    const { data, boundary } = encodeMultipart(parts);
    return { data, mimeType: `multipart/mixed; boundary=${boundary}` };
  }

  /** Reflective state: plain, non-helper, non-SQLite fields of `this` (the live `config` accessor is never snapshotted). */
  private reflectiveState(): Record<string, unknown> {
    const state: Record<string, unknown> = {};
    for (const [k, val] of Object.entries(this.instance)) {
      if (typeof val === 'function') continue;
      if (k === 'config') continue;
      if (
        isInstance(val, DatabaseSync) ||
        isInstance(val, StatementSync) ||
        isInstance(val, Session) ||
        isInstance(val, SQLTagStore)
      ) {
        continue;
      }
      state[k] = val;
    }
    return state;
  }

  async loadSnapshot(bytes: Uint8Array, mimeType?: string): Promise<void> {
    if (this.customSnapshot) {
      await this.customSnapshot.load.call(this.instance, bytes);
      return;
    }
    const applyState = async (json: string): Promise<void> => {
      let parsed = JSON.parse(json) as Record<string, unknown>;
      if (this.reg.snapshotStateSchema) {
        parsed = await validateSnapshotState(this.reg.snapshotStateSchema, parsed);
      } else {
        delete parsed.config; // never let a stale snapshot clobber the live config accessor
      }
      Object.assign(this.instance, parsed);
    };

    if (mimeType && mimeType.startsWith('multipart/mixed')) {
      const boundary = mimeType.match(/boundary=([^\s;]+)/)?.[1];
      if (!boundary) throw 'multipart/mixed snapshot missing boundary parameter';
      const parts = decodeMultipart(bytes, boundary);
      const statePart = parts.find((p) => p.name === 'state');
      if (!statePart) throw 'multipart snapshot missing "state" part';
      await applyState(new TextDecoder().decode(statePart.body));
      for (const p of parts) {
        if (!p.name.startsWith('db:')) continue;
        const field = this.instance[p.name.slice(3)];
        if (isInstance(field, DatabaseSync)) restoreDatabaseSync(field, p.body);
      }
      return;
    }
    await applyState(new TextDecoder().decode(bytes));
  }
}

/**
 * Validate + scope a snapshot state object through the declared Standard Schema.
 * Undeclared fields (`config`, `getId`, …) are stripped, so only the declared
 * state is persisted/restored; a shape mismatch throws.
 */
/** `val instanceof Ctor`, tolerant of an undefined constructor (e.g. node:sqlite absent). */
function isInstance(val: unknown, Ctor: unknown): boolean {
  return typeof Ctor === 'function' && val instanceof (Ctor as new (...args: never[]) => unknown);
}

async function validateSnapshotState(
  schema: StandardSchemaV1,
  value: unknown,
): Promise<Record<string, unknown>> {
  let result = schema['~standard'].validate(value);
  if (result instanceof Promise) result = await result;
  if (result.issues) {
    throw `snapshot state does not match its declared schema: ${result.issues
      .map((i) => i.message)
      .join('; ')}`;
  }
  return result.value as Record<string, unknown>;
}

/** Register the agent's initiator. On `initiate`, decode id, run `init`, wire handlers. */
export function registerAgentInitiator(
  reg: RegisteredAgent,
  impl: AgentImplementation<IdRecord, MethodsRecord, ConfigSpec, object>,
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

      // Fresh-reading config accessor; shared by `init` (via context) and the
      // handler `this`. Each getter re-fetches on access (config may change
      // between invocations).
      const config = buildConfigAccessor(reg.configDeclarations);

      // `init` may be synchronous or async (return a Promise); awaiting a plain
      // value is a no-op, so both forms work. The guest `initialize`/load-snapshot
      // paths await the initiate result.
      let state: object;
      try {
        state = await impl.init({
          id: idRecord as never,
          principal: sdkPrincipal,
          phantomId,
          config,
        });
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
      instance.config = config;

      return {
        tag: 'ok',
        val: new FluentResolvedAgent(
          reg,
          instance,
          impl.methods as Record<string, (...args: unknown[]) => unknown>,
          agentId,
          impl.snapshot,
        ) as never,
      };
    },
  });
}
