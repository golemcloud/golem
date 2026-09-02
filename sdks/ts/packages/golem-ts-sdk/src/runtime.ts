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

// Self-contained SDK runtime, built ONLY on the new schema model: it compiles
// id/method schemas to `SchemaCodec`s, assembles the WIT `AgentType` via
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
  schemaValueToWitAsync,
} from './internal/schema-model';
import { AgentClassName } from './agentClassName';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import { AgentInitiatorRegistry } from './internal/registry/agentInitiatorRegistry';
import { getRawSelfAgentId } from './host/hostapi';
import { createCustomError, invalidInput, invalidMethod } from './internal/agentError';
import { sdkPrincipalFromHost } from './principal';
import { ParsedAgentId } from './agentId';
import {
  DatabaseSync,
  Session,
  SQLTagStore,
  StatementSync,
  isAutocommitDatabaseSync,
  restoreDatabaseSync,
  serializeDatabaseSync,
} from './internal/sqlite';
import { encodeMultipart, MultipartPart } from './internal/multipart';
import { compileSchema } from './schema/adapter';
import { SchemaCodec } from './schema/codec';
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
import {
  buildConfigAccessor,
  collectConfigLeaves,
  compileConfigTree,
  ConfigDeclaration,
  ConfigGroupNode,
  ConfigSpec,
} from './config';
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
  codec: SchemaCodec;
}

/** A compiled method: ordered input codecs + a unit-or-single output + metadata. */
interface MethodCodec {
  name: string;
  inputCodecs: NamedCodec[];
  output: { tag: 'unit' } | { tag: 'single'; codec: SchemaCodec };
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
  /** Presence tree driving the runtime config accessor (optional-group aware). */
  configTree: ConfigGroupNode;
  /**
   * Typed snapshot-state schema from `snapshotting: { state }`. When set, the
   * JSON snapshot is scoped to + validated by this schema. Without it,
   * snapshotting requires custom save/load functions.
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
  if (AgentTypeRegistry.exists(className)) {
    throw new Error(`Agent "${name}" is already registered`);
  }
  AgentTypeRegistry.beginRegistration(className);

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

  const configTree = compileConfigTree(metadata.config);
  const configDeclarations = collectConfigLeaves(configTree);

  const agentType = assembleAgentType(name, idCodecs, methodCodecs, configDeclarations, metadata);
  AgentTypeRegistry.completeRegistration(className, agentType);

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
    configTree,
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

/** Map the {@link SnapshottingSpec} to the WIT `snapshotting` variant. */
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
 * Registry-free consistency checks for the HTTP surface:
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

  // A bare `s.principal()` parameter is auto-injected from the caller (the host
  // supplies the `Principal`, so it carries NO wire field); every other parameter
  // is user-supplied. Matches the base SDK's `auto-injected(principal)` source.
  const encodeInput = (codecs: NamedCodec[]): InputSchema => ({
    tag: 'parameters',
    val: codecs.map((c) => ({
      name: c.name,
      source:
        c.codec.autoInjected === 'principal'
          ? { tag: 'auto-injected', val: 'principal' }
          : { tag: 'user-supplied' },
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
 * `saveSnapshot`.
 */
class ResolvedAgentImpl {
  constructor(
    private readonly reg: RegisteredAgent,
    /** The handler `this`: state fields + `getId`/`getPhantomId` helpers. */
    private readonly instance: Record<string, unknown>,
    private readonly methods: Record<string, (...args: unknown[]) => unknown>,
    private readonly agentId: ParsedAgentId,
    /** Optional user-supplied snapshot serializer (`implement({ snapshot })`). */
    private readonly customSnapshot?: {
      save: () => Uint8Array | Promise<Uint8Array>;
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
    principal: HostPrincipal,
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
        // The wire record carries ONE field per user-supplied parameter, in
        // declaration order; an auto-injected `s.principal()` parameter has NO
        // wire field and is filled from the separate `principal` arg. Walk with a
        // cursor so user-supplied decoding stays aligned (mirrors the base SDK's
        // `decodeInputRecord`). When every parameter is auto-injected the wire
        // record is empty, so only read `methodArgs` when a user-supplied field exists.
        const hasUserSupplied = mc.inputCodecs.some((ic) => ic.codec.autoInjected !== 'principal');
        const fields = hasUserSupplied
          ? (schemaValueFromWit(methodArgs) as Extract<SchemaValue, { tag: 'record' }>).fields
          : [];
        const record: Record<string, unknown> = {};
        let cursor = 0;
        for (const ic of mc.inputCodecs) {
          record[ic.name] =
            ic.codec.autoInjected === 'principal'
              ? sdkPrincipalFromHost(principal)
              : ic.codec.fromValue(fields[cursor++]);
        }
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
      return { tag: 'ok', val: await schemaValueToWitAsync(mc.output.codec.toValue(result)) };
    } catch (e) {
      return {
        tag: 'err',
        val: createCustomError(`Failed to encode result of ${methodName}: ${errorMessage(e)}`),
      };
    }
  }

  // Snapshot serialization. Two modes:
  //  - custom (`implement({ snapshot })`): user save/load own the bytes verbatim.
  //  - typed  (`snapshotting: { state }`): JSON of ONLY the schema-validated state
  //           fields of `this`, plus a `db:<field>` SQLite part per DatabaseSync.
  // The principal/version envelope is added by the guest (`src/index.ts`).
  async saveSnapshot(): Promise<{ data: Uint8Array; mimeType: string }> {
    if (this.customSnapshot) {
      const data = await this.customSnapshot.save.call(this.instance);
      return { data, mimeType: 'application/octet-stream' };
    }
    if (!this.reg.snapshotStateSchema) {
      throw 'snapshot saving requires a declared state schema or custom save/load functions';
    }

    const databases: Array<{ name: string; bytes: Uint8Array }> = [];
    const ordinaryState: Record<string, unknown> = {};
    const seen = new Set<unknown>();
    for (const [k, val] of Object.entries(this.instance)) {
      if (k === 'config' || k === 'getId' || k === 'getPhantomId' || k === 'getPrincipal') continue;
      if (isDatabaseSync(val)) {
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
      if (
        isInstance(val, StatementSync) ||
        isInstance(val, Session) ||
        isInstance(val, SQLTagStore)
      ) {
        throw `Cannot automatically snapshot resource field "${k}"; use custom save/load functions.`;
      }
      assertNoNestedSnapshotResources(val, k);
      ordinaryState[k] = val;
    }

    const state = await validateSnapshotState(this.reg.snapshotStateSchema, ordinaryState, true);
    assertJsonSnapshotValue(state, 'state');

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
}

function isDatabaseSync(val: unknown): val is DatabaseSync {
  return val instanceof DatabaseSync;
}

/** `val instanceof Ctor`, including builtins whose constructors are not public. */
function isInstance(val: unknown, Ctor: Function): boolean {
  return typeof Ctor === 'function' && val instanceof Ctor;
}

function assertNoNestedSnapshotResources(
  value: unknown,
  path: string,
  ancestors = new Set<object>(),
): void {
  if (typeof value === 'function') {
    throw `Cannot automatically snapshot function field "${path}"; use custom save/load functions.`;
  }
  if (value === null || typeof value !== 'object') return;
  if (
    isDatabaseSync(value) ||
    isInstance(value, StatementSync) ||
    isInstance(value, Session) ||
    isInstance(value, SQLTagStore)
  ) {
    throw `Cannot automatically snapshot nested resource field "${path}"; use custom save/load functions.`;
  }
  if (ancestors.has(value)) return;
  ancestors.add(value);
  for (const [key, nested] of Object.entries(value)) {
    assertNoNestedSnapshotResources(nested, `${path}.${key}`, ancestors);
  }
  ancestors.delete(value);
}

function assertJsonSnapshotValue(
  value: unknown,
  path: string,
  ancestors = new Set<object>(),
): void {
  if (
    value === undefined ||
    typeof value === 'function' ||
    typeof value === 'symbol' ||
    typeof value === 'bigint'
  ) {
    throw `Snapshot ${path} is not JSON-deserializable; use custom save/load functions.`;
  }
  if (typeof value === 'number' && !Number.isFinite(value)) {
    throw `Snapshot ${path} is not JSON-deserializable; use custom save/load functions.`;
  }
  if (value === null || typeof value !== 'object') return;
  if (ancestors.has(value)) {
    throw `Snapshot ${path} contains a cycle; use custom save/load functions.`;
  }
  if (!Array.isArray(value) && Object.getPrototypeOf(value) !== Object.prototype) {
    throw `Snapshot ${path} is not a plain JSON value; use custom save/load functions.`;
  }
  ancestors.add(value);
  for (const [key, nested] of Object.entries(value)) {
    assertJsonSnapshotValue(nested, `${path}.${key}`, ancestors);
  }
  ancestors.delete(value);
}

/**
 * Validate a snapshot state object through the declared Standard Schema.
 * Shape mismatches are rejected. During serialization, fields removed by the
 * schema are also rejected as undeclared state.
 */
async function validateSnapshotState(
  schema: StandardSchemaV1,
  value: unknown,
  rejectUndeclaredFields = false,
): Promise<Record<string, unknown>> {
  let result = schema['~standard'].validate(value);
  if (result instanceof Promise) result = await result;
  if (result.issues) {
    throw `snapshot state does not match its declared schema: ${result.issues
      .map((i) => i.message)
      .join('; ')}`;
  }
  const restored = result.value as Record<string, unknown>;
  if (rejectUndeclaredFields && value !== null && typeof value === 'object') {
    const declared = new Set(Object.keys(restored));
    const undeclared = Object.keys(value).filter((key) => !declared.has(key));
    if (undeclared.length > 0) {
      throw `snapshot state contains undeclared fields: ${undeclared.join(', ')}`;
    }
  }
  return restored;
}

/** Register the agent's initiator. On `initiate`, decode id, run `init`, wire handlers. */
export function registerAgentInitiator(
  reg: RegisteredAgent,
  impl: AgentImplementation<IdRecord, MethodsRecord, ConfigSpec, object>,
): void {
  if (AgentInitiatorRegistry.exists(reg.name)) {
    throw new Error(`Agent "${reg.name}" already has an implementation`);
  }
  const resolveContext = (
    constructorInput: SchemaValue,
    principal: HostPrincipal,
  ):
    | { tag: 'err'; val: AgentError }
    | {
        tag: 'ok';
        val: {
          idRecord: Record<string, unknown>;
          agentId: ParsedAgentId;
          phantomId: ReturnType<ParsedAgentId['parsed']>[2];
          sdkPrincipal: ReturnType<typeof sdkPrincipalFromHost>;
          config: ReturnType<typeof buildConfigAccessor>;
        };
      } => {
    let idRecord: Record<string, unknown>;
    try {
      // Same cursor-based decode as `invoke`: an auto-injected `s.principal()`
      // id field is filled from the separate `principal` arg and consumes no
      // wire field. For the common all-user-supplied case this is identical to
      // a positional read.
      const hasUserSupplied = reg.idCodecs.some((ic) => ic.codec.autoInjected !== 'principal');
      const fields = hasUserSupplied
        ? (constructorInput as Extract<SchemaValue, { tag: 'record' }>).fields
        : [];
      idRecord = {};
      let cursor = 0;
      for (const ic of reg.idCodecs) {
        idRecord[ic.name] =
          ic.codec.autoInjected === 'principal'
            ? sdkPrincipalFromHost(principal)
            : ic.codec.fromValue(fields[cursor++]);
      }
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
    const config = buildConfigAccessor(reg.configTree);

    return {
      tag: 'ok',
      val: { idRecord, agentId, phantomId, sdkPrincipal, config },
    };
  };

  const complete = (
    state: object,
    context: Extract<ReturnType<typeof resolveContext>, { tag: 'ok' }>['val'],
  ): ResolvedAgentImpl => {
    const { agentId, phantomId, sdkPrincipal, config } = context;
    const instance: Record<string, unknown> = { ...(state as Record<string, unknown>) };
    instance.getId = () => agentId;
    instance.getPhantomId = () => phantomId;
    instance.getPrincipal = () => sdkPrincipal;
    instance.config = config;
    return new ResolvedAgentImpl(
      reg,
      instance,
      impl.methods as Record<string, (...args: unknown[]) => unknown>,
      agentId,
      impl.snapshot,
    );
  };

  AgentInitiatorRegistry.register(reg.className, {
    async initiate(constructorInput: SchemaValue, principal: HostPrincipal) {
      const resolved = resolveContext(constructorInput, principal);
      if (resolved.tag === 'err') return resolved;
      const { idRecord, phantomId, sdkPrincipal, config } = resolved.val;

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

      return {
        tag: 'ok',
        val: complete(state, resolved.val) as never,
      };
    },
    async loadSnapshot(constructorInput, principal, bytes, _mimeType, databases) {
      const resolved = resolveContext(constructorInput, principal);
      if (resolved.tag === 'err') return resolved;
      const { idRecord, phantomId, sdkPrincipal, config } = resolved.val;
      let state: object;
      try {
        if (impl.snapshot?.load) {
          const load = impl.snapshot.load;
          state = await load(bytes, {
            id: idRecord as never,
            agentId: resolved.val.agentId,
            principal: sdkPrincipal,
            phantomId,
            config,
          });
        } else if (reg.snapshotStateSchema) {
          state = await validateSnapshotState(
            reg.snapshotStateSchema,
            JSON.parse(new TextDecoder().decode(bytes)),
          );
        } else {
          throw new Error('snapshot restoration is not configured');
        }

        const restored = state as Record<string, unknown>;
        for (const database of databases) {
          let target = restored[database.name];
          if (target === undefined) {
            target = new DatabaseSync(':memory:');
            restored[database.name] = target;
          }
          if (!isDatabaseSync(target)) {
            throw new Error(`snapshot database field "${database.name}" is not a DatabaseSync`);
          }
          restoreDatabaseSync(target, database.bytes);
        }
      } catch (e) {
        return {
          tag: 'err',
          val: createCustomError(`Agent ${reg.name} restoration failed: ${errorMessage(e)}`),
        };
      }
      return { tag: 'ok', val: complete(state, resolved.val) as never };
    },
  });
}
