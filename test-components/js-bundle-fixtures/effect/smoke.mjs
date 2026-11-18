import { Context, Layer, Effect, Exit, Schema, Scope, Stream, Pull, Cause, SchemaAST, SchemaGetter, SchemaIssue, HashMap, Result, Option, DateTime, Pipeable, Duration, Predicate, Logger, References, Scheduler, Tracer, ManagedRuntime } from 'effect';
import { existsSync } from 'node:fs';
import * as CoreTypes from 'golem:core/types@2.0.0';
import { uuidToString, parseUuid } from 'golem:core/types@2.0.0';
import * as AgentHost from 'golem:agent/host@2.0.0';
import { WasmRpc } from 'golem:agent/host@2.0.0';
import * as ApiHost from 'golem:api/host@1.5.0';
import * as WasiEnv from 'wasi:cli/environment@0.3.0';
import * as Blob from 'wasi:blobstore/blobstore';
import * as Types from 'wasi:blobstore/types';
import * as Runtime from 'wasm-rquickjs:custom-invocation-context';
import * as DurabilityHost from 'golem:durability/durability@1.6.0';
import * as Host from 'golem:agent/durable-streams@2.0.0';
import { DbConnection } from 'golem:rdbms/ignite2@1.5.0';
import * as InvocationContext from 'golem:agent/invocation-context@2.0.0';
import * as KvBatch from 'wasi:keyvalue/eventual-batch@0.1.0';
import * as KvEventual from 'wasi:keyvalue/eventual@0.1.0';
import * as KvTypes from 'wasi:keyvalue/types@0.1.0';
import * as WasiLogging from 'wasi:logging/logging';
import { DbConnection as DbConnection$1 } from 'golem:rdbms/mysql@1.5.0';
import * as NodeSqlite from 'node:sqlite';
import { DatabaseSync } from 'node:sqlite';
import * as OplogHost from 'golem:api/oplog@1.5.0';
import * as DeriveHost from 'golem:permissions/derive@0.1.0';
import * as InspectHost from 'golem:permissions/inspect@0.1.0';
import * as KernelHost from 'golem:permissions/kernel-introspection@0.1.0';
import * as RevokeHost from 'golem:permissions/revoke@0.1.0';
import * as TypesHost from 'golem:permissions/types@0.1.0';
import * as WalletHost from 'golem:permissions/wallet@0.1.0';
import { DbConnection as DbConnection$2 } from 'golem:rdbms/postgres@1.5.0';
import * as QuotaHost from 'golem:quota/types@1.5.0';
import * as RetryHost from 'golem:api/retry@1.5.0';
import * as RetryScopeHost from 'golem:api/retry-scope@1.5.0';
import process from 'node:process';
import * as RevealHost from 'golem:secrets/reveal@0.1.0';
import * as SecretsHost from 'golem:secrets/types@0.1.0';
import * as Host$1 from 'golem:tool/host@0.1.0';
import * as ContextHost from 'golem:api/context@1.5.0';
import * as WsClient from 'golem:websocket/client@1.5.0';

/**
 * Host service bundling the agent-metadata + webhook subset of two
 * related WIT interfaces:
 *
 * - `golem:agent/host@2.0.0.parseAgentId` — recovers a structured
 *   `(typeName, ctorDataValue, phantomId?)` tuple from the
 *   `GOLEM_AGENT_ID` env var. Used by the snapshot-load dispatcher.
 * - `golem:agent/host@2.0.0.createWebhook` — mints a public POST URL
 *   bound to a previously-allocated host promise. Used by
 *   `src/webhook.ts:create`.
 * - `golem:api/host@1.5.0.getSelfMetadata` — reads the running agent's
 *   full metadata. Used by the uncached `SelfAgentId` effect and by
 *   `src/agents.ts:getSelfMetadata` for user-facing reads.
 * - `golem:api/host@1.5.0.getAgentMetadata` / `updateAgent` /
 *   `forkAgent` / `revertAgent` / `fork` — agent-lifecycle calls used
 *   by `src/agents.ts`.
 * - `golem:api/host@1.5.0.resolveComponentId` / `resolveAgentId` /
 *   `resolveAgentIdStrict` — id-resolution helpers used by
 *   `src/agents.ts`.
 * - `golem:api/host@1.5.0.GetAgents` — paged-iterator constructor used
 *   by `src/agents.ts:getAgents`.
 *
 * Pragmatically grouped here because they all answer the same
 * "who am I?" / "where am I reachable?" / "who else is around?"
 * question, even though several live in `golem:api/host` rather than
 * `golem:agent/host`. The eventual plan splits these further into
 * `ParseAgentIdClient` + `WebhookClient` + `AgentMetadataClient`; for
 * now the bundle keeps the wiring footprint small.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class AgentHostClient extends Context.Service()("effect-golem/host/AgentHost") {
}
const AgentHostLive = Layer.succeed(AgentHostClient, AgentHostClient.of({
    makeAgentId: (name, input, phantomId) => AgentHost.makeAgentId(name, input, phantomId),
    getAgentType: (name) => AgentHost.getAgentType(name),
    getAgentTypeByAgentId: (agentId) => AgentHost.getAgentTypeByAgentId(agentId),
    getAllAgentTypes: () => AgentHost.getAllAgentTypes(),
    parseAgentId: (agentId) => AgentHost.parseAgentId(agentId),
    getSelfMetadata: () => ApiHost.getSelfMetadata(),
    createWebhook: (promiseId) => AgentHost.createWebhook(promiseId),
    getAgentMetadata: (id) => ApiHost.getAgentMetadata(id),
    updateAgent: (id, t, m) => ApiHost.updateAgent(id, t, m),
    forkAgent: (s, t, o) => ApiHost.forkAgent(s, t, o),
    revertAgent: (id, t) => ApiHost.revertAgent(id, t),
    fork: () => ApiHost.fork(),
    resolveComponentId: (r) => ApiHost.resolveComponentId(r),
    resolveAgentId: (r, n) => ApiHost.resolveAgentId(r, n),
    resolveAgentIdStrict: (r, n) => ApiHost.resolveAgentIdStrict(r, n),
    getAgentsCtor: (c, f, p) => new ApiHost.GetAgents(c, f, p),
    trap: (reason) => {
        ApiHost.trap(reason);
        // The line above never returns at runtime (the host call is an
        // uncatchable wasm trap). The throw keeps `never` happy on the
        // type level for environments where the WIT binding is replaced
        // by a no-op stub (tests).
        throw new Error(`golem:api/host.trap returned (reason: ${reason})`);
    },
}));

/** Thin injectable façade over the schema-native agent RPC host. */
class RpcHostError {
    cause;
    operation;
    _tag = "RpcHostError";
    constructor(cause, operation) {
        this.cause = cause;
        this.operation = operation;
    }
}
class RpcClient extends Context.Service()("effect-golem/host/Rpc") {
}
const dispose = (resource) => {
    try {
        ;
        resource[Symbol.dispose]?.();
    }
    catch {
        // Resource release is best-effort after the operation has completed.
    }
};
const wrapWasmRpc = (rpc) => ({
    invokeAndAwait: (methodName, input, scopeCard) => rpc.invokeAndAwait(methodName, input, scopeCard),
    invoke: (methodName, input) => rpc.invoke(methodName, input, undefined),
    asyncInvokeAndAwait: (methodName, input, scopeCard) => {
        const invocation = rpc.asyncInvokeAndAwait(methodName, input, scopeCard);
        return {
            metadata: invocation.metadata,
            get: () => invocation.future.get(),
            cancel: () => invocation.future.cancel(),
            drop: () => dispose(invocation.future),
        };
    },
    scheduleInvocation: (scheduledAt, methodName, input) => rpc.scheduleInvocation(scheduledAt, methodName, input, undefined),
    scheduleCancelableInvocation: (scheduledAt, methodName, input) => {
        const receipt = rpc.scheduleCancelableInvocation(scheduledAt, methodName, input, undefined);
        return {
            metadata: receipt.metadata,
            token: {
                cancel: () => receipt.cancellationToken.cancel(),
                drop: () => dispose(receipt.cancellationToken),
            },
        };
    },
    drop: () => dispose(rpc),
});
const RpcLive = Layer.succeed(RpcClient, RpcClient.of({
    bindSelf: () => Effect.try({
        try: () => {
            const [id, rpc] = WasmRpc.bindSelf();
            return [id, wrapWasmRpc(rpc)];
        },
        catch: (cause) => new RpcHostError(cause, "WasmRpc.bindSelf"),
    }),
    connect: (agentTypeName, constructorValue, phantomId, agentConfig) => Effect.try({
        try: () => wrapWasmRpc(WasmRpc.create(agentTypeName, constructorValue, phantomId, [...agentConfig])),
        catch: (cause) => new RpcHostError(cause, "WasmRpc.create"),
    }),
}));

const prototype = Object.freeze(Object.create(Object.prototype, {
    toString: {
        value() {
            return `${this.agentId}.${this.uuid}`;
        },
    },
}));
/** Build an immutable invocation ID whose `toString` returns `<agent-id>.<uuid>`. */
const makeInvocationId = (agentId, uuid) => Object.freeze(Object.assign(Object.create(prototype), { agentId, uuid }));

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
// Scope cards never enter schema values, so only the bundle that contains
// `Permissions` and the RPC clients sees them. The state stays module-local.
const states$3 = new WeakMap();
const isScopeCard = (value) => typeof value === "object" && value !== null && states$3.has(value);
/** The raw resource of a held scope card, or `undefined` when it was released. */
const peekScopeCard = (card) => {
    const state = states$3.get(card);
    if (state === undefined)
        throw new TypeError("not a scope card");
    return state.raw;
};

const pending = new WeakSet();
function assertCapabilityReady(handle) {
    if (pending.has(handle))
        throw new Error("capability validation is still in progress");
}
/** One conversion owns its rollback actions, including across Effect suspension. */
class CapabilityTransaction {
    rollbacks = [];
    handles = [];
    lock(handle) {
        if (pending.has(handle))
            throw new Error("capability is already being validated");
        pending.add(handle);
        this.handles.push(handle);
    }
    register(rollback) {
        this.rollbacks.push(rollback);
    }
    commit(retainRollback) {
        for (const handle of this.handles)
            pending.delete(handle);
        this.handles.length = 0;
        if (retainRollback && this.rollbacks.length > 0) {
            const rollbacks = this.rollbacks.splice(0);
            retainRollback(() => {
                for (let i = rollbacks.length - 1; i >= 0; i--)
                    rollbacks[i]();
                rollbacks.length = 0;
            });
        }
        this.rollbacks.length = 0;
    }
    rollback() {
        for (const handle of this.handles)
            pending.delete(handle);
        this.handles.length = 0;
        for (let i = this.rollbacks.length - 1; i >= 0; i--) {
            this.rollbacks[i]();
        }
        this.rollbacks.length = 0;
    }
}
function withCapabilityAdoptionTransaction(convert, retainRollback) {
    const transaction = new CapabilityTransaction();
    try {
        const result = convert(transaction);
        transaction.commit(retainRollback);
        return result;
    }
    catch (error) {
        transaction.rollback();
        throw error;
    }
}
const withCapabilityTransaction = (convert) => Effect.suspend(() => {
    const transaction = new CapabilityTransaction();
    return Effect.onExit(Effect.suspend(() => convert(transaction)), (exit) => Effect.sync(() => {
        if (Exit.isSuccess(exit))
            transaction.commit();
        else
            transaction.rollback();
    }));
});

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
// Capability key gating the privileged secret operations that wrap, borrow, or
// transfer the underlying owned `own<secret>` resource.
const SECRET_INTERNAL = Symbol("golem-secret-internal");

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
const registryKey$2 = Symbol.for("@effect-golem/secret-handle-registry");
const globals$2 = globalThis;
const registry$3 = (globals$2[registryKey$2] ??= {
    states: new WeakMap(),
    owners: new WeakMap(),
    wireOrigins: new WeakMap(),
    transferredOwner: Object.freeze({}),
});
const { states: states$2, owners: owners$2, wireOrigins: wireOrigins$2, transferredOwner: transferredOwner$2 } = registry$3;
class GuestSecretHandle {
    constructor(key, raw, onTake, tracked = true) {
        if (key !== SECRET_INTERNAL) {
            throw new Error("GuestSecretHandle construction is an internal SDK operation");
        }
        states$2.set(this, { raw, onTake, tracked });
    }
    toJSON() {
        throw new Error("secret handles cannot be serialized; transfer them through a WIT schema-value-tree");
    }
}
function requireSecretInternal(key) {
    if (key !== SECRET_INTERNAL) {
        throw new Error("this is an internal SDK operation on a secret handle");
    }
}
function stateOf$2(handle) {
    const state = states$2.get(handle);
    if (state === undefined) {
        throw new Error("invalid secret handle");
    }
    return state;
}
function isGuestSecretHandle(value) {
    return typeof value === "object" && value !== null && states$2.has(value);
}
function peekGuestSecretHandle(key, handle) {
    requireSecretInternal(key);
    return stateOf$2(handle).raw;
}
function takeGuestSecretHandleToWire(key, handle, wireOwner) {
    requireSecretInternal(key);
    assertCapabilityReady(handle);
    const state = stateOf$2(handle);
    const raw = state.raw;
    if (raw !== undefined && state.tracked && owners$2.get(raw) !== handle) {
        throw new Error("secret handle ownership is invalid");
    }
    state.raw = undefined;
    if (raw !== undefined) {
        if (state.tracked) {
            owners$2.set(raw, wireOwner);
            wireOrigins$2.set(wireOwner, handle);
        }
        state.onTake?.();
    }
    return raw;
}
/**
 * Give a raw secret that `wireOwner` still holds back to the handle it was taken from. Use it only
 * when the host never received the wire value. See `restoreGuestPermissionCardHandleFromWire`.
 */
function restoreGuestSecretHandleFromWire(key, raw, wireOwner) {
    requireSecretInternal(key);
    const handle = wireOrigins$2.get(wireOwner);
    if (handle === undefined || owners$2.get(raw) !== wireOwner)
        return "unknown";
    const state = stateOf$2(handle);
    if (state.raw !== undefined)
        return "unknown";
    wireOrigins$2.delete(wireOwner);
    if (state.released) {
        owners$2.delete(raw);
        return "released";
    }
    state.raw = raw;
    owners$2.set(raw, handle);
    return "restored";
}
function assertGuestSecretHandleCanLiftFromWire(key, raw, wireOwner) {
    requireSecretInternal(key);
    const owner = owners$2.get(raw);
    if (owner !== undefined && owner !== wireOwner) {
        throw new Error("secret handle is already owned");
    }
}
function liftGuestSecretHandleFromWire(key, raw, wireOwner) {
    assertGuestSecretHandleCanLiftFromWire(key, raw, wireOwner);
    const handle = new GuestSecretHandle(key, raw);
    owners$2.set(raw, handle);
    return handle;
}

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
// Capability key gating the privileged quota-token operations that move or
// re-wrap the underlying owned `own<quota-token>` resource.
//
// A `quota-token` is an unforgeable, affine capability: a guest may hold and
// transfer it but must never extract the raw owned handle, re-wrap it, or
// duplicate it — doing so would let it forge or double-spend the capability.
// The privileged holder and bridge conversion functions all require this key
// as a witness, so only SDK-internal modules that can import it may call them.
//
// This module is intentionally NOT re-exported from the package's public entry
// point (`src/index.ts`) nor from the `internal/schema-model` barrel, so guest
// code cannot obtain the symbol. A `Symbol()` is unique and unguessable, so it
// cannot be forged at runtime either.
const QUOTA_INTERNAL = Symbol("golem:quota internal capability");

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
const registryKey$1 = Symbol.for("@effect-golem/quota-token-handle-registry");
const globals$1 = globalThis;
const registry$2 = (globals$1[registryKey$1] ??= {
    states: new WeakMap(),
    owners: new WeakMap(),
    wireOrigins: new WeakMap(),
    transferredOwner: Object.freeze({}),
});
const { states: states$1, owners: owners$1, wireOrigins: wireOrigins$1, transferredOwner: transferredOwner$1 } = registry$2;
class GuestQuotaTokenHandle {
    constructor(key, raw, tracked = true) {
        if (key !== QUOTA_INTERNAL) {
            throw new Error("GuestQuotaTokenHandle construction is an internal SDK operation");
        }
        states$1.set(this, { raw, tracked });
    }
    /**
     * Quota-token handles are unforgeable capabilities, not data: serializing one
     * (e.g. via `JSON.stringify`) is always an error. Transfer them only by
     * passing the owning `QuotaToken` through a WIT `schema-value-tree`.
     */
    toJSON() {
        throw new Error("quota-token handles cannot be serialized; transfer them through a WIT schema-value-tree");
    }
}
function requireQuotaInternal(key) {
    if (key !== QUOTA_INTERNAL) {
        throw new Error("this is an internal SDK operation on a quota-token handle");
    }
}
function stateOf$1(handle) {
    const state = states$1.get(handle);
    if (state === undefined) {
        throw new Error("invalid quota-token handle");
    }
    return state;
}
function isGuestQuotaTokenHandle(value) {
    return typeof value === "object" && value !== null && states$1.has(value);
}
function peekGuestQuotaTokenHandle(key, handle) {
    requireQuotaInternal(key);
    return stateOf$1(handle).raw;
}
function takeGuestQuotaTokenHandleToWire(key, handle, wireOwner) {
    requireQuotaInternal(key);
    assertCapabilityReady(handle);
    const state = stateOf$1(handle);
    const raw = state.raw;
    if (raw !== undefined && state.tracked && owners$1.get(raw) !== handle) {
        throw new Error("quota-token handle ownership is invalid");
    }
    state.raw = undefined;
    if (raw !== undefined && state.tracked) {
        owners$1.set(raw, wireOwner);
        wireOrigins$1.set(wireOwner, handle);
    }
    return raw;
}
/**
 * Give a raw token that `wireOwner` still holds back to the handle it was taken from. Use it only
 * when the host never received the wire value. See `restoreGuestPermissionCardHandleFromWire`.
 */
function restoreGuestQuotaTokenHandleFromWire(key, raw, wireOwner) {
    requireQuotaInternal(key);
    const handle = wireOrigins$1.get(wireOwner);
    if (handle === undefined || owners$1.get(raw) !== wireOwner)
        return "unknown";
    const state = stateOf$1(handle);
    if (state.raw !== undefined)
        return "unknown";
    wireOrigins$1.delete(wireOwner);
    if (state.released) {
        owners$1.delete(raw);
        return "released";
    }
    state.raw = raw;
    owners$1.set(raw, handle);
    return "restored";
}
function assertGuestQuotaTokenHandleCanLiftFromWire(key, raw, wireOwner) {
    requireQuotaInternal(key);
    const owner = owners$1.get(raw);
    if (owner !== undefined && owner !== wireOwner) {
        throw new Error("quota-token handle is already owned");
    }
}
function liftGuestQuotaTokenHandleFromWire(key, raw, wireOwner) {
    assertGuestQuotaTokenHandleCanLiftFromWire(key, raw, wireOwner);
    const handle = new GuestQuotaTokenHandle(key, raw);
    owners$1.set(raw, handle);
    return handle;
}
function abandonGuestQuotaTokenWireHandle(key, raw, wireOwner) {
    requireQuotaInternal(key);
    const owner = owners$1.get(raw);
    if (owner === undefined || owner === wireOwner) {
        owners$1.set(raw, transferredOwner$1);
    }
}

const STREAM_INTERNAL = Symbol("golem:stream internal capability");

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
/** A typed producer failure, separate from per-item Result values. */
class StreamError extends Schema.TaggedError()("StreamError", {
    failure: Schema.Unknown,
}) {
    constructor(failure) {
        super({ failure });
        this.message =
            failure.tag === "failed" && failure.val.value.tag === "string"
                ? failure.val.value.value
                : "val" in failure && typeof failure.val === "string"
                    ? failure.val
                    : `agent stream ${failure.tag}`;
        this.name = failure.tag === "cancelled" ? "AbortError" : "StreamError";
    }
    static failed(value) {
        return new StreamError({ tag: "failed", val: value });
    }
    static from(error) {
        return error instanceof StreamError
            ? error
            : StreamError.failed({
                graph: { defs: new Map(), root: t.string() },
                value: v.string(error instanceof Error ? error.message : String(error)),
            });
    }
}
/** Raised when encoding an in-memory schema/value into its flat WIT carrier fails. */
class SchemaEncodeError extends Error {
    constructor(message) {
        super(message);
        this.name = "SchemaEncodeError";
    }
}
/** Raised when decoding a flat WIT carrier into the in-memory model fails. */
class SchemaDecodeError extends Error {
    constructor(message) {
        super(message);
        this.name = "SchemaDecodeError";
    }
}
/**
 * Raised when merging per-root schema graphs encounters two definitions that
 * share the same `type-id` but have structurally different bodies.
 */
class SchemaConflictError extends Error {
    typeId;
    constructor(typeId, message) {
        super(message ?? `conflicting definitions for type id '${typeId}'`);
        this.typeId = typeId;
        this.name = "SchemaConflictError";
    }
}

async function wrapSchemaValueStream(source) {
    let resolve;
    const outcome = new Promise((done) => {
        resolve = done;
    });
    const items = {
        [Symbol.asyncIterator]() {
            let iterator;
            let closed = false;
            return {
                async next() {
                    if (closed)
                        return { done: true, value: undefined };
                    try {
                        iterator ??= source[Symbol.asyncIterator]();
                        const item = await iterator.next();
                        if (!item.done)
                            return item;
                        closed = true;
                        resolve({ tag: "ok", val: undefined });
                    }
                    catch (error) {
                        closed = true;
                        const failure = StreamError.from(error).failure;
                        resolve({
                            tag: "err",
                            val: failure.tag === "failed"
                                ? {
                                    tag: "failed",
                                    val: {
                                        graph: schemaGraphToWit(failure.val.graph),
                                        value: await schemaValueToWitAsync(failure.val.value),
                                    },
                                }
                                : failure,
                        });
                    }
                    return { done: true, value: undefined };
                },
                async return() {
                    if (!closed) {
                        closed = true;
                        resolve(undefined);
                        iterator ??= source[Symbol.asyncIterator]();
                        await iterator.return?.();
                    }
                    return { done: true, value: undefined };
                },
            };
        },
    };
    const { SchemaValueStream } = await import('golem:core/types@2.0.0');
    return SchemaValueStream.wrap(items, outcome);
}
async function unwrapSchemaValueStream(value) {
    const { SchemaValueStream } = await import('golem:core/types@2.0.0');
    const [items, outcome] = await SchemaValueStream.unwrap(value);
    return {
        [Symbol.asyncIterator]() {
            const iterator = items[Symbol.asyncIterator]();
            let closed = false;
            return {
                async next() {
                    if (closed)
                        return { done: true, value: undefined };
                    const item = await iterator.next();
                    if (!item.done)
                        return item;
                    closed = true;
                    const result = await outcome;
                    if (result.tag === "err") {
                        const failure = result.val;
                        throw new StreamError(failure.tag === "failed"
                            ? {
                                tag: "failed",
                                val: typedSchemaValueFromWit(failure.val),
                            }
                            : failure);
                    }
                    return { done: true, value: undefined };
                },
                async return() {
                    closed = true;
                    await iterator.return?.();
                    return { done: true, value: undefined };
                },
            };
        },
    };
}
class GuestSchemaValueStreamHandle {
    #value;
    #ownership;
    #onTake;
    #onRelease;
    constructor(key, value, onTake, onRelease, ownership = { available: true }) {
        if (key !== STREAM_INTERNAL) {
            throw new Error("GuestSchemaValueStreamHandle construction is an internal SDK operation");
        }
        this.#value = value;
        this.#ownership = ownership;
        this.#onTake = onTake;
        this.#onRelease = onRelease;
    }
    ownership(key) {
        if (key !== STREAM_INTERNAL)
            throw new Error("stream ownership is internal");
        return this.#ownership;
    }
    peek() {
        return this.#ownership.available ? this.#value : undefined;
    }
    reserve(owner) {
        assertCapabilityReady(this.#ownership);
        if (this.#ownership.reader)
            throw new Error("schema stream already has a reader");
        if (this.#ownership.busy)
            throw new Error("a schema stream operation is in progress");
        if (this.#ownership.reservation !== undefined || this.peek() === undefined) {
            throw new Error("schema stream is already reserved or transferred");
        }
        this.#ownership.reservation = owner;
    }
    unreserve(owner) {
        if (this.#ownership.reservation === owner)
            this.#ownership.reservation = undefined;
    }
    take(owner) {
        assertCapabilityReady(this.#ownership);
        if (this.#ownership.reader)
            throw new Error("schema stream already has a reader");
        if (this.#ownership.busy)
            throw new Error("a schema stream operation is in progress");
        if (this.#ownership.reservation !== owner)
            throw new Error("schema stream is reserved for transfer");
        this.#ownership.reservation = undefined;
        const value = this.peek();
        this.#ownership.available = false;
        this.#value = undefined;
        if (value !== undefined)
            this.#onTake?.();
        return value;
    }
    release() {
        assertCapabilityReady(this.#ownership);
        if (this.#ownership.reader)
            throw new Error("schema stream already has a reader");
        if (this.#ownership.busy)
            throw new Error("a schema stream operation is in progress");
        if (this.#ownership.reservation !== undefined)
            throw new Error("schema stream is reserved for transfer");
        if (this.peek() !== undefined) {
            this.#ownership.available = false;
            this.#value = undefined;
            this.#onRelease?.();
        }
    }
}

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
// Capability key gating the privileged operations that move or re-wrap an
// owned `permission-card` resource. This module is intentionally not exported
// from the package entry point or the schema-model barrel.
const PERMISSION_CARD_INTERNAL = Symbol("golem:permission-card internal capability");

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
// The package ships the main entry and the middleware entry as separate bundles, and both lift
// and lower permission cards. They must recognise each other's cards, so the registry lives on
// `globalThis` under a well-known symbol instead of in module scope. Code in the same component
// can therefore reach the registry and alias one raw resource into several card objects, or
// read the raw resource of a card. It cannot create authority: the raw resource comes only from
// the host, and the host checks every use of it. Aliasing makes the second use of a moved
// resource fail on the host.
const registryKey = Symbol.for("@effect-golem/permission-card-handle-registry");
const globals = globalThis;
const registry$1 = (globals[registryKey] ??= {
    states: new WeakMap(),
    owners: new WeakMap(),
    wireOrigins: new WeakMap(),
    transferredOwner: Object.freeze({}),
});
const { states, owners, wireOrigins, transferredOwner } = registry$1;
/**
 * Guest-side take-once carrier for an owned, opaque permission-card resource. `K` is the card
 * kind; a handle of one kind is not assignable to a handle of the other kind.
 */
class GuestPermissionCardHandle {
    constructor(key, raw, tracked = true, kind) {
        if (key !== PERMISSION_CARD_INTERNAL) {
            throw new Error("GuestPermissionCardHandle construction is an internal SDK operation");
        }
        states.set(this, { raw, tracked, kind });
    }
    toJSON() {
        throw new Error("permission-card handles cannot be serialized; transfer them through a WIT schema-value-tree");
    }
}
function requirePermissionCardInternal(key) {
    if (key !== PERMISSION_CARD_INTERNAL) {
        throw new Error("this is an internal SDK operation on a permission-card handle");
    }
}
function stateOf(handle) {
    const state = states.get(handle);
    if (state === undefined) {
        throw new Error("invalid permission-card handle");
    }
    return state;
}
function isGuestPermissionCardHandle(value) {
    return (typeof value === "object" && value !== null && states.has(value));
}
/** Whether `value` is a card of `kind`. A received card whose kind is not known yet is not. */
function isGuestPermissionCardHandleOfKind(value, kind) {
    return isGuestPermissionCardHandle(value) && states.get(value).kind === kind;
}
/** The kind of the card, or `undefined` for a received card that no schema has typed yet. */
function guestPermissionCardHandleKind(handle) {
    return stateOf(handle).kind;
}
/**
 * Type a card as `kind`. A received card whose kind is not known yet takes `kind`; a card of the
 * other kind is rejected, so no card changes its kind.
 */
function assignGuestPermissionCardHandleKind(key, handle, kind) {
    requirePermissionCardInternal(key);
    const state = stateOf(handle);
    if (state.kind === undefined)
        state.kind = kind;
    else if (state.kind !== kind) {
        throw new Error(kind === "template"
            ? "a concrete permission card is not a polymorphic permission card"
            : "a polymorphic permission card is not a concrete permission card");
    }
    return handle;
}
function peekGuestPermissionCardHandle(key, handle) {
    requirePermissionCardInternal(key);
    return stateOf(handle).raw;
}
function takeGuestPermissionCardHandleToWire(key, handle, wireOwner) {
    requirePermissionCardInternal(key);
    assertCapabilityReady(handle);
    const state = stateOf(handle);
    const raw = state.raw;
    if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
        throw new Error("permission-card handle ownership is invalid");
    }
    state.raw = undefined;
    if (raw !== undefined && state.tracked) {
        owners.set(raw, wireOwner);
        wireOrigins.set(wireOwner, handle);
    }
    return raw;
}
/**
 * Give a raw resource that `wireOwner` still holds back to the card it was taken from. Use it
 * only when the host never received the wire value. Returns `"released"` when the card was
 * released while the resource was on the wire; the caller must then drop the resource. Returns
 * `"unknown"` when the resource has no known origin or the origin card holds another resource.
 */
function restoreGuestPermissionCardHandleFromWire(key, raw, wireOwner) {
    requirePermissionCardInternal(key);
    const handle = wireOrigins.get(wireOwner);
    if (handle === undefined || owners.get(raw) !== wireOwner)
        return "unknown";
    const state = stateOf(handle);
    if (state.raw !== undefined)
        return "unknown";
    wireOrigins.delete(wireOwner);
    if (state.released) {
        owners.delete(raw);
        return "released";
    }
    state.raw = raw;
    owners.set(raw, handle);
    return "restored";
}
function assertGuestPermissionCardHandleCanLiftFromWire(key, raw, wireOwner) {
    requirePermissionCardInternal(key);
    const owner = owners.get(raw);
    if (owner !== undefined && owner !== wireOwner) {
        throw new Error("permission-card handle is already owned");
    }
}
function liftGuestPermissionCardHandleFromWire(key, raw, wireOwner) {
    assertGuestPermissionCardHandleCanLiftFromWire(key, raw, wireOwner);
    // A lifted card has no kind; the schema that decodes it assigns the declared kind. The host
    // keeps the real kind of the card, so a decoded kind gives no authority.
    const handle = new GuestPermissionCardHandle(key, raw);
    owners.set(raw, handle);
    return handle;
}
function abandonGuestPermissionCardWireHandle(key, raw, wireOwner) {
    requirePermissionCardInternal(key);
    const owner = owners.get(raw);
    if (owner === undefined || owner === wireOwner) {
        owners.set(raw, transferredOwner);
    }
}

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
function resolveShapeType(graph, type) {
    let current = type;
    const seen = new Set();
    while (current.body.tag === "ref") {
        if (seen.has(current.body.id))
            return undefined;
        seen.add(current.body.id);
        const definition = graph.defs.get(current.body.id);
        if (!definition)
            return undefined;
        current = definition.body;
    }
    return current.body;
}
// ============================================================
// Constructors / helpers
// ============================================================
function emptyMetadata() {
    return { aliases: [], examples: [] };
}
/** Wrap a body into a `SchemaType` node with empty metadata. */
function schemaType(body, metadata = emptyMetadata()) {
    return { body, metadata };
}
/** Compact constructors for schema types (anonymous unless wrapped in a def). */
const t = {
    ref: (id) => schemaType({ tag: "ref", id }),
    bool: () => schemaType({ tag: "bool" }),
    s8: (restrictions) => schemaType({ tag: "s8", restrictions }),
    s16: (restrictions) => schemaType({ tag: "s16", restrictions }),
    s32: (restrictions) => schemaType({ tag: "s32", restrictions }),
    s64: (restrictions) => schemaType({ tag: "s64", restrictions }),
    u8: (restrictions) => schemaType({ tag: "u8", restrictions }),
    u16: (restrictions) => schemaType({ tag: "u16", restrictions }),
    u32: (restrictions) => schemaType({ tag: "u32", restrictions }),
    u64: (restrictions) => schemaType({ tag: "u64", restrictions }),
    f32: (restrictions) => schemaType({ tag: "f32", restrictions }),
    f64: (restrictions) => schemaType({ tag: "f64", restrictions }),
    char: () => schemaType({ tag: "char" }),
    string: () => schemaType({ tag: "string" }),
    record: (fields) => schemaType({ tag: "record", fields }),
    variant: (cases) => schemaType({ tag: "variant", cases }),
    enum: (cases) => schemaType({ tag: "enum", cases }),
    flags: (names) => schemaType({ tag: "flags", names }),
    tuple: (elements) => schemaType({ tag: "tuple", elements }),
    list: (element) => schemaType({ tag: "list", element }),
    fixedList: (element, length) => schemaType({ tag: "fixed-list", element, length }),
    map: (key, value) => schemaType({ tag: "map", key, value }),
    option: (element) => schemaType({ tag: "option", element }),
    result: (ok, err) => schemaType({ tag: "result", ok, err }),
    text: (restrictions = {}) => schemaType({ tag: "text", restrictions }),
    binary: (restrictions = {}) => schemaType({ tag: "binary", restrictions }),
    path: (spec) => schemaType({ tag: "path", spec }),
    url: (restrictions) => schemaType({ tag: "url", restrictions }),
    uuid: () => schemaType({ tag: "uuid" }),
    datetime: () => schemaType({ tag: "datetime" }),
    duration: () => schemaType({ tag: "duration" }),
    quantity: (spec) => schemaType({ tag: "quantity", spec }),
    union: (branches) => schemaType({ tag: "union", branches }),
    secret: (inner, spec = {}) => schemaType({ tag: "secret", spec, inner }),
    quotaToken: (spec) => schemaType({ tag: "quota-token", spec }),
    future: (element) => schemaType({ tag: "future", element }),
    stream: (element) => schemaType({ tag: "stream", element }),
    permissionCard: (spec) => schemaType({ tag: "permission-card", spec }),
};
/** Compact constructors for schema field/case helpers. */
function field(name, body, metadata = emptyMetadata()) {
    return { name, body, metadata };
}
function variantCase(name, payload, metadata = emptyMetadata()) {
    return { name, payload, metadata };
}
/** Compact constructors for schema values. */
const v = {
    bool: (value) => ({ tag: "bool", value }),
    s8: (value) => ({ tag: "s8", value }),
    s16: (value) => ({ tag: "s16", value }),
    s32: (value) => ({ tag: "s32", value }),
    s64: (value) => ({ tag: "s64", value }),
    u8: (value) => ({ tag: "u8", value }),
    u16: (value) => ({ tag: "u16", value }),
    u32: (value) => ({ tag: "u32", value }),
    u64: (value) => ({ tag: "u64", value }),
    f32: (value) => ({ tag: "f32", value }),
    f64: (value) => ({ tag: "f64", value }),
    char: (value) => ({ tag: "char", value }),
    string: (value) => ({ tag: "string", value }),
    record: (fields) => ({ tag: "record", fields }),
    variant: (caseIndex, payload) => ({
        tag: "variant",
        caseIndex,
        payload,
    }),
    enum: (caseIndex) => ({ tag: "enum", caseIndex }),
    flags: (flags) => ({ tag: "flags", flags }),
    tuple: (elements) => ({ tag: "tuple", elements }),
    list: (elements) => ({ tag: "list", elements }),
    fixedList: (elements) => ({ tag: "fixed-list", elements }),
    map: (entries) => ({ tag: "map", entries }),
    option: (value) => ({ tag: "option", value }),
    ok: (value) => ({ tag: "result", result: { tag: "ok", value } }),
    err: (value) => ({ tag: "result", result: { tag: "err", value } }),
    text: (text, language) => ({ tag: "text", text, language }),
    binary: (bytes, mimeType) => ({
        tag: "binary",
        bytes,
        mimeType,
    }),
    path: (value) => ({ tag: "path", value }),
    url: (value) => ({ tag: "url", value }),
    uuid: (value) => ({ tag: "uuid", value }),
    datetime: (value) => ({ tag: "datetime", value }),
    duration: (nanoseconds) => ({ tag: "duration", nanoseconds }),
    quantity: (value) => ({ tag: "quantity", value }),
    union: (unionTag, body) => ({ tag: "union", unionTag, body }),
    secret: (handle) => ({ tag: "secret", handle }),
    quotaToken: (handle) => ({ tag: "quota-token", handle }),
    stream: (handle) => ({ tag: "stream", handle }),
    permissionCard: (handle) => ({
        tag: "permission-card",
        handle,
    }),
};
/** Clone a schema value without duplicating affine capability handles. */
function cloneSchemaValue(value) {
    switch (value.tag) {
        case "record":
            return { tag: "record", fields: value.fields.map(cloneSchemaValue) };
        case "variant":
            return {
                tag: "variant",
                caseIndex: value.caseIndex,
                payload: value.payload ? cloneSchemaValue(value.payload) : undefined,
            };
        case "flags":
            return { tag: "flags", flags: [...value.flags] };
        case "tuple":
            return { tag: "tuple", elements: value.elements.map(cloneSchemaValue) };
        case "list":
            return { tag: "list", elements: value.elements.map(cloneSchemaValue) };
        case "fixed-list":
            return { tag: "fixed-list", elements: value.elements.map(cloneSchemaValue) };
        case "map":
            return {
                tag: "map",
                entries: value.entries.map((entry) => ({
                    key: cloneSchemaValue(entry.key),
                    value: cloneSchemaValue(entry.value),
                })),
            };
        case "option":
            return {
                tag: "option",
                value: value.value !== undefined ? cloneSchemaValue(value.value) : undefined,
            };
        case "result":
            return {
                tag: "result",
                result: {
                    tag: value.result.tag,
                    value: value.result.value !== undefined ? cloneSchemaValue(value.result.value) : undefined,
                },
            };
        case "binary":
            return { ...value, bytes: value.bytes.slice() };
        case "uuid":
            return { ...value, value: { ...value.value } };
        case "datetime":
            return { ...value, value: { ...value.value } };
        case "quantity":
            return { ...value, value: { ...value.value } };
        case "union":
            return { tag: "union", unionTag: value.unionTag, body: cloneSchemaValue(value.body) };
        default:
            return { ...value };
    }
}
// ============================================================
// Structural equality (bigint / Uint8Array / Map aware)
// ============================================================
/**
 * Deep structural equality that understands the value shapes used by the schema
 * model: bigint, Uint8Array, Map, arrays, and plain objects. Object keys whose
 * value is `undefined` are ignored, matching the WIT option lifting convention
 * (and Vitest's `toEqual`).
 */
function deepEqual(a, b, equivalent) {
    // Numbers use `Object.is` so that `NaN` equals `NaN` and, crucially, `-0` does
    // NOT equal `0` (a real f32/f64 round-trip difference we must not mask).
    if (typeof a === "number" && typeof b === "number") {
        return Object.is(a, b);
    }
    if (a === b)
        return true;
    // Capability handles are affine, not structural data: equality is identity
    // only. Without this, distinct handles would compare equal because they
    // expose no enumerable state.
    if (isGuestSecretHandle(a) || isGuestSecretHandle(b))
        return false;
    if (a instanceof GuestQuotaTokenHandle || b instanceof GuestQuotaTokenHandle)
        return false;
    if (a instanceof GuestSchemaValueStreamHandle || b instanceof GuestSchemaValueStreamHandle)
        return false;
    if (isGuestPermissionCardHandle(a) || isGuestPermissionCardHandle(b)) {
        return false;
    }
    if (typeof a === "bigint" || typeof b === "bigint")
        return a === b;
    if (a === null || b === null || typeof a !== "object" || typeof b !== "object") {
        return false;
    }
    if (a instanceof Uint8Array || b instanceof Uint8Array) {
        if (!(a instanceof Uint8Array) || !(b instanceof Uint8Array))
            return false;
        if (a.length !== b.length)
            return false;
        for (let i = 0; i < a.length; i++)
            if (a[i] !== b[i])
                return false;
        return true;
    }
    if (a instanceof Map || b instanceof Map) {
        if (!(a instanceof Map) || !(b instanceof Map))
            return false;
        if (a.size !== b.size)
            return false;
        for (const [k, av] of a) {
            if (!b.has(k))
                return false;
            if (!deepEqual(av, b.get(k)))
                return false;
        }
        return true;
    }
    if (Array.isArray(a) || Array.isArray(b)) {
        if (!Array.isArray(a) || !Array.isArray(b))
            return false;
        if (a.length !== b.length)
            return false;
        for (let i = 0; i < a.length; i++) {
            if (!deepEqual(a[i], b[i]))
                return false;
        }
        return true;
    }
    const ao = a;
    const bo = b;
    const keys = new Set();
    for (const k of Object.keys(ao))
        if (ao[k] !== undefined)
            keys.add(k);
    for (const k of Object.keys(bo))
        if (bo[k] !== undefined)
            keys.add(k);
    for (const k of keys) {
        if (!deepEqual(ao[k], bo[k]))
            return false;
    }
    return true;
}

/** A transport producer that cannot touch its source before ownership handoff. */
class PreparedStream {
    source;
    state = "pending";
    wake;
    ready = new Promise((resolve) => {
        this.wake = resolve;
    });
    iterator;
    busy = false;
    constructor(source) {
        this.source = source;
    }
    get pending() {
        return this.state === "pending";
    }
    commit() {
        if (this.state !== "pending")
            throw new Error("stream preparation is no longer pending");
        this.state = "committed";
        this.wake();
    }
    abort() {
        if (this.state !== "pending")
            return;
        this.state = "aborted";
        this.wake();
    }
    [Symbol.asyncIterator]() {
        return this;
    }
    async next() {
        if (this.busy)
            throw new Error("a stream read is already in progress");
        this.busy = true;
        try {
            await this.ready;
            if (this.state !== "committed")
                return { done: true, value: undefined };
            this.iterator ??= this.source[Symbol.asyncIterator]();
            return await this.iterator.next();
        }
        finally {
            this.busy = false;
        }
    }
    async return() {
        const committed = this.state === "committed";
        this.state = "closed";
        this.wake();
        if (committed) {
            this.iterator ??= this.source[Symbol.asyncIterator]();
            await this.iterator.return?.();
        }
        return { done: true, value: undefined };
    }
}

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
// ============================================================
// Schema type / graph
// ============================================================
// Numeric `SchemaType` variants (`s8`..`f64`) carry an inline
// `option<numeric-restrictions>`. The model type and the WIT carrier are the
// same generated `NumericRestrictions` shape, so the codec normalizes in both
// directions to mirror the Rust `NumericRestrictions::normalize` invariants: an
// empty restriction set (no bounds, empty/absent unit) collapses to `undefined`
// (`none`), an empty `unit` is dropped, and a float bound of `-0.0` is
// canonicalized to `+0.0` bits so equal restrictions compare equal.
const NUMERIC_BOUND_BITS_VIEW = new DataView(new ArrayBuffer(8));
/** Canonicalize a numeric bound: collapse `-0.0` float bits to `+0.0` bits. */
function canonicalizeNumericBound(bound) {
    if (bound.tag === "float-bits") {
        NUMERIC_BOUND_BITS_VIEW.setBigUint64(0, BigInt.asUintN(64, bound.val), true);
        if (NUMERIC_BOUND_BITS_VIEW.getFloat64(0, true) === 0) {
            return { tag: "float-bits", val: 0n };
        }
    }
    return bound;
}
/**
 * Normalize an `option<numeric-restrictions>` carrier, returning `undefined`
 * (WIT `none`) for the empty restriction set. Used for both encode (model →
 * WIT) and decode (WIT → model) since the two shapes are identical.
 */
function normalizeNumericRestrictions(restrictions) {
    if (restrictions === undefined) {
        return undefined;
    }
    const out = {};
    if (restrictions.min !== undefined) {
        out.min = canonicalizeNumericBound(restrictions.min);
    }
    if (restrictions.max !== undefined) {
        out.max = canonicalizeNumericBound(restrictions.max);
    }
    if (restrictions.unit !== undefined && restrictions.unit !== "") {
        out.unit = restrictions.unit;
    }
    if (out.min === undefined && out.max === undefined && out.unit === undefined) {
        return undefined;
    }
    return out;
}
/**
 * Incremental encoder for a single flat {@link WitSchemaGraph} that holds
 * several independent root types in one shared `type-nodes` pool. Mirrors the
 * Rust `golem-schema` `GraphEncoder`.
 *
 * Seed the encoder with the agent's merged named definitions, then call
 * {@link GraphEncoder.encodeType} once per inline root (constructor / method /
 * config), collecting the returned `type-node-index` values, and finally
 * {@link GraphEncoder.finish} to obtain the graph with a placeholder root. The
 * graph's own `root` is a structurally-required placeholder (an empty record);
 * agent-layer carriers never consult it — the real roots are the returned
 * indices.
 *
 * Do NOT encode each agent root via {@link schemaGraphToWit} and reuse those
 * indices: per-graph node indices are only valid within a single encoding.
 */
class GraphEncoder {
    typeNodes = [];
    defIndexById = new Map();
    witDefs;
    constructor(defs) {
        // Deterministic def ordering (sorted by id); `ref-type` carries the def index.
        const ids = [...defs.keys()].sort();
        ids.forEach((id, i) => this.defIndexById.set(id, i));
        // Reserve def slots first so `ref-type` resolves forward references during
        // body encoding, then fill each body.
        this.witDefs = ids.map((id) => ({ id, name: defs.get(id).name, body: -1 }));
        ids.forEach((id, i) => {
            this.witDefs[i].body = this.encodeType(defs.get(id).body);
        });
    }
    /** Flatten one (possibly recursive) schema type into the shared pool and return its index. */
    encodeType(st) {
        const body = this.encodeBody(st.body);
        this.typeNodes.push({ body, metadata: st.metadata });
        return this.typeNodes.length - 1;
    }
    encodeBody(body) {
        switch (body.tag) {
            case "ref": {
                const di = this.defIndexById.get(body.id);
                if (di === undefined) {
                    throw new SchemaEncodeError(`schema graph references unknown type id '${body.id}'`);
                }
                return { tag: "ref-type", val: di };
            }
            case "bool":
                return { tag: "bool-type" };
            case "s8":
                return { tag: "s8-type", val: normalizeNumericRestrictions(body.restrictions) };
            case "s16":
                return { tag: "s16-type", val: normalizeNumericRestrictions(body.restrictions) };
            case "s32":
                return { tag: "s32-type", val: normalizeNumericRestrictions(body.restrictions) };
            case "s64":
                return { tag: "s64-type", val: normalizeNumericRestrictions(body.restrictions) };
            case "u8":
                return { tag: "u8-type", val: normalizeNumericRestrictions(body.restrictions) };
            case "u16":
                return { tag: "u16-type", val: normalizeNumericRestrictions(body.restrictions) };
            case "u32":
                return { tag: "u32-type", val: normalizeNumericRestrictions(body.restrictions) };
            case "u64":
                return { tag: "u64-type", val: normalizeNumericRestrictions(body.restrictions) };
            case "f32":
                return { tag: "f32-type", val: normalizeNumericRestrictions(body.restrictions) };
            case "f64":
                return { tag: "f64-type", val: normalizeNumericRestrictions(body.restrictions) };
            case "char":
                return { tag: "char-type" };
            case "string":
                return { tag: "string-type" };
            case "record":
                return {
                    tag: "record-type",
                    val: body.fields.map((f) => ({
                        name: f.name,
                        body: this.encodeType(f.body),
                        metadata: f.metadata,
                    })),
                };
            case "variant":
                return {
                    tag: "variant-type",
                    val: body.cases.map((c) => ({
                        name: c.name,
                        payload: c.payload !== undefined ? this.encodeType(c.payload) : undefined,
                        metadata: c.metadata,
                    })),
                };
            case "enum":
                return { tag: "enum-type", val: body.cases };
            case "flags":
                return { tag: "flags-type", val: body.names };
            case "tuple":
                return { tag: "tuple-type", val: body.elements.map((e) => this.encodeType(e)) };
            case "list":
                return { tag: "list-type", val: this.encodeType(body.element) };
            case "fixed-list":
                return {
                    tag: "fixed-list-type",
                    val: { element: this.encodeType(body.element), length: body.length },
                };
            case "map":
                return {
                    tag: "map-type",
                    val: { key: this.encodeType(body.key), value: this.encodeType(body.value) },
                };
            case "option":
                return { tag: "option-type", val: this.encodeType(body.element) };
            case "result":
                return {
                    tag: "result-type",
                    val: {
                        ok: body.ok !== undefined ? this.encodeType(body.ok) : undefined,
                        err: body.err !== undefined ? this.encodeType(body.err) : undefined,
                    },
                };
            case "text":
                return { tag: "text-type", val: body.restrictions };
            case "binary":
                return { tag: "binary-type", val: body.restrictions };
            case "path":
                return { tag: "path-type", val: body.spec };
            case "url":
                return { tag: "url-type", val: body.restrictions };
            case "uuid":
                return { tag: "uuid-type" };
            case "datetime":
                return { tag: "datetime-type" };
            case "duration":
                return { tag: "duration-type" };
            case "quantity":
                return { tag: "quantity-type", val: body.spec };
            case "union":
                return {
                    tag: "union-type",
                    val: {
                        branches: body.branches.map((br) => ({
                            tag: br.tag,
                            body: this.encodeType(br.body),
                            discriminator: br.discriminator,
                            metadata: br.metadata,
                        })),
                    },
                };
            case "secret":
                return { tag: "secret-type", val: { ...body.spec, inner: this.encodeType(body.inner) } };
            case "quota-token":
                return { tag: "quota-token-type", val: body.spec };
            case "permission-card":
                return { tag: "permission-card-type", val: body.spec };
            case "future":
                return {
                    tag: "future-type",
                    val: body.element !== undefined ? this.encodeType(body.element) : undefined,
                };
            case "stream":
                return {
                    tag: "stream-type",
                    val: body.element !== undefined ? this.encodeType(body.element) : undefined,
                };
            default:
                throw new SchemaEncodeError(`unknown schema type body tag '${body.tag}'`);
        }
    }
    /** Encode `root` as the graph root and return the finished flat graph. */
    encodeGraphRoot(root) {
        const rootIdx = this.encodeType(root);
        return { typeNodes: this.typeNodes, defs: this.witDefs, root: rootIdx };
    }
    /**
     * Finish the graph with a placeholder empty-record root. Use after collecting
     * the real root indices via {@link GraphEncoder.encodeType}.
     */
    finish() {
        const root = this.encodeType({
            body: { tag: "record", fields: [] },
            metadata: emptyMetadata(),
        });
        return { typeNodes: this.typeNodes, defs: this.witDefs, root };
    }
}
function schemaGraphToWit(graph) {
    return new GraphEncoder(graph.defs).encodeGraphRoot(graph.root);
}
/** Decode a Component Model carrier into the recursive SDK model. */
function schemaGraphFromWit(wit) {
    const decoded = schemaGraphRootsFromWit(wit, [wit.root]);
    return { defs: decoded.defs, root: decoded.roots[0] };
}
/** Decode selected roots once against one shared definition pool. */
function schemaGraphRootsFromWit(wit, roots) {
    const nodes = wit.typeNodes;
    const witDefs = wit.defs;
    // See `schemaValueFromWit`: a flat on-path `Uint8Array` (`1` = on the current
    // DFS path) replaces a `Set<number>` + `try/finally`, with identical
    // cycle-detection semantics. Legitimate recursion goes through `ref-type`
    // (which resolves to a def id without recursing here), so only a structural
    // back-edge in raw type-node indices is reported as a cycle.
    const onPath = new Uint8Array(nodes.length);
    const decoded = new Array(nodes.length);
    function idByDefIndex(di) {
        if (di < 0 || di >= witDefs.length) {
            throw new SchemaDecodeError(`def index out of range: ${di} (defs: ${witDefs.length})`);
        }
        return witDefs[di].id;
    }
    function fromType(idx) {
        if (idx < 0 || idx >= nodes.length) {
            throw new SchemaDecodeError(`type node index out of range: ${idx} (nodes: ${nodes.length})`);
        }
        if (onPath[idx] === 1) {
            throw new SchemaDecodeError(`cyclic type node reference at index ${idx}`);
        }
        if (decoded[idx] !== undefined)
            return decoded[idx];
        onPath[idx] = 1;
        const node = nodes[idx];
        const result = { body: fromBody(node.body), metadata: node.metadata };
        onPath[idx] = 0;
        decoded[idx] = result;
        return result;
    }
    function fromBody(body) {
        switch (body.tag) {
            case "ref-type":
                return { tag: "ref", id: idByDefIndex(body.val) };
            case "bool-type":
                return { tag: "bool" };
            case "s8-type":
                return { tag: "s8", restrictions: normalizeNumericRestrictions(body.val) };
            case "s16-type":
                return { tag: "s16", restrictions: normalizeNumericRestrictions(body.val) };
            case "s32-type":
                return { tag: "s32", restrictions: normalizeNumericRestrictions(body.val) };
            case "s64-type":
                return { tag: "s64", restrictions: normalizeNumericRestrictions(body.val) };
            case "u8-type":
                return { tag: "u8", restrictions: normalizeNumericRestrictions(body.val) };
            case "u16-type":
                return { tag: "u16", restrictions: normalizeNumericRestrictions(body.val) };
            case "u32-type":
                return { tag: "u32", restrictions: normalizeNumericRestrictions(body.val) };
            case "u64-type":
                return { tag: "u64", restrictions: normalizeNumericRestrictions(body.val) };
            case "f32-type":
                return { tag: "f32", restrictions: normalizeNumericRestrictions(body.val) };
            case "f64-type":
                return { tag: "f64", restrictions: normalizeNumericRestrictions(body.val) };
            case "char-type":
                return { tag: "char" };
            case "string-type":
                return { tag: "string" };
            case "record-type":
                return {
                    tag: "record",
                    fields: body.val.map((f) => ({
                        name: f.name,
                        body: fromType(f.body),
                        metadata: f.metadata,
                    })),
                };
            case "variant-type":
                return {
                    tag: "variant",
                    cases: body.val.map((c) => ({
                        name: c.name,
                        payload: c.payload !== undefined ? fromType(c.payload) : undefined,
                        metadata: c.metadata,
                    })),
                };
            case "enum-type":
                return { tag: "enum", cases: body.val };
            case "flags-type":
                return { tag: "flags", names: body.val };
            case "tuple-type":
                return { tag: "tuple", elements: body.val.map((i) => fromType(i)) };
            case "list-type":
                return { tag: "list", element: fromType(body.val) };
            case "fixed-list-type":
                return { tag: "fixed-list", element: fromType(body.val.element), length: body.val.length };
            case "map-type":
                return { tag: "map", key: fromType(body.val.key), value: fromType(body.val.value) };
            case "option-type":
                return { tag: "option", element: fromType(body.val) };
            case "result-type":
                return {
                    tag: "result",
                    ok: body.val.ok !== undefined ? fromType(body.val.ok) : undefined,
                    err: body.val.err !== undefined ? fromType(body.val.err) : undefined,
                };
            case "text-type":
                return { tag: "text", restrictions: body.val };
            case "binary-type":
                return { tag: "binary", restrictions: body.val };
            case "path-type":
                return { tag: "path", spec: body.val };
            case "url-type":
                return { tag: "url", restrictions: body.val };
            case "uuid-type":
                return { tag: "uuid" };
            case "datetime-type":
                return { tag: "datetime" };
            case "duration-type":
                return { tag: "duration" };
            case "quantity-type":
                return { tag: "quantity", spec: body.val };
            case "union-type":
                return {
                    tag: "union",
                    branches: body.val.branches.map((br) => ({
                        tag: br.tag,
                        body: fromType(br.body),
                        discriminator: br.discriminator,
                        metadata: br.metadata,
                    })),
                };
            case "secret-type":
                return {
                    tag: "secret",
                    spec: { category: body.val.category },
                    inner: fromType(body.val.inner),
                };
            case "quota-token-type":
                return { tag: "quota-token", spec: body.val };
            case "permission-card-type":
                return { tag: "permission-card", spec: body.val };
            case "future-type":
                return { tag: "future", element: body.val !== undefined ? fromType(body.val) : undefined };
            case "stream-type":
                return { tag: "stream", element: body.val !== undefined ? fromType(body.val) : undefined };
            default:
                throw new SchemaDecodeError(`unknown schema type body tag '${body.tag}'`);
        }
    }
    const defs = new Map();
    for (const d of witDefs) {
        if (defs.has(d.id)) {
            throw new SchemaDecodeError(`duplicate def id '${d.id}' in schema graph`);
        }
        defs.set(d.id, { name: d.name, body: fromType(d.body) });
    }
    const selected = roots.map(fromType);
    return { defs, roots: selected };
}
// ============================================================
// Schema value
// ============================================================
// Inclusive ranges for the WIT integer scalars, used by the encode preflight to
// reject out-of-range values before any owned handle is moved (see
// `assertSchemaValueRepresentable`). These mirror the Component Model boundary, which would
// otherwise throw *after* a sibling quota-token handle was already transferred.
const INT_RANGES = {
    s8: [-128, 127],
    s16: [-32768, 32767],
    s32: [-2147483648, 2147483647],
    u8: [0, 255],
    u16: [0, 65535],
    u32: [0, 4294967295],
};
const S64_MIN$1 = -(2n ** 63n);
const S64_MAX$1 = 2n ** 63n - 1n;
const U64_MAX$3 = 2n ** 64n - 1n;
function checkIntRange(tag, value) {
    const [min, max] = INT_RANGES[tag];
    if (!Number.isInteger(value) || value < min || value > max) {
        throw new SchemaEncodeError(`${tag} value out of range: ${value}`);
    }
}
function assertDenseModelArray(value, name) {
    if (!Array.isArray(value)) {
        throw new SchemaEncodeError(`${name} must be an array`);
    }
    for (let i = 0; i < value.length; i++) {
        if (!(i in value)) {
            throw new SchemaEncodeError(`${name} must be a dense array`);
        }
    }
}
/**
 * Validate every node of `value` *before* {@link schemaValueToWit} moves any
 * owned capability handle, so a value tree that the WIT boundary would reject
 * never destroys a still-valid capability.
 *
 * This enforces the same invariants the Component Model boundary and the host
 * decoder enforce — narrow-integer ranges, valid `s64`/`u64` magnitudes, a
 * single-code-point non-surrogate `char`, a signed-`s64` duration, and a
 * `datetime` whose nanoseconds are in `[0, 1_000_000_000)` — plus that every
 * owned capability is still present and that no underlying owned resource
 * appears more than once. Handles are deduplicated by the identity of the
 * underlying owned resource (peeked without consuming), not merely by holder
 * identity, so two holders that somehow wrap the same resource are also rejected.
 */
function assertSchemaValueRepresentable(value, allowNativeStreams = false, seen = new Set()) {
    const visit = (v) => {
        switch (v.tag) {
            case "s8":
            case "s16":
            case "s32":
            case "u8":
            case "u16":
            case "u32":
                checkIntRange(v.tag, v.value);
                return;
            case "s64":
                if (typeof v.value !== "bigint" || v.value < S64_MIN$1 || v.value > S64_MAX$1) {
                    throw new SchemaEncodeError(`s64 value out of range: ${v.value}`);
                }
                return;
            case "u64":
                if (typeof v.value !== "bigint" || v.value < 0n || v.value > U64_MAX$3) {
                    throw new SchemaEncodeError(`u64 value out of range: ${v.value}`);
                }
                return;
            case "bool":
                if (typeof v.value !== "boolean") {
                    throw new SchemaEncodeError(`bool value must be a boolean: ${String(v.value)}`);
                }
                return;
            case "f32":
            case "f64":
                if (typeof v.value !== "number") {
                    throw new SchemaEncodeError(`${v.tag} value must be a number: ${String(v.value)}`);
                }
                return;
            case "char": {
                const codePoints = [...v.value];
                const cp = codePoints.length === 1 ? codePoints[0].codePointAt(0) : undefined;
                if (cp === undefined || (cp >= 0xd800 && cp <= 0xdfff)) {
                    throw new SchemaEncodeError(`char value must be a single Unicode scalar value: ${JSON.stringify(v.value)}`);
                }
                return;
            }
            case "string":
            case "path":
            case "url":
                if (typeof v.value !== "string") {
                    throw new SchemaEncodeError(`${v.tag} value must be a string: ${String(v.value)}`);
                }
                return;
            case "uuid":
                if (typeof v.value.highBits !== "bigint" ||
                    typeof v.value.lowBits !== "bigint" ||
                    v.value.highBits < 0n ||
                    v.value.highBits > (1n << 64n) - 1n ||
                    v.value.lowBits < 0n ||
                    v.value.lowBits > (1n << 64n) - 1n) {
                    throw new SchemaEncodeError("uuid value must contain two unsigned 64-bit integers");
                }
                return;
            case "datetime": {
                const ns = v.value.nanoseconds;
                if (!Number.isInteger(ns) || ns < 0 || ns >= 1_000_000_000) {
                    throw new SchemaEncodeError(`invalid datetime value: nanoseconds must be in [0, 1_000_000_000), got ${ns}`);
                }
                if (typeof v.value.seconds !== "bigint" ||
                    v.value.seconds < S64_MIN$1 ||
                    v.value.seconds > S64_MAX$1) {
                    throw new SchemaEncodeError(`datetime seconds out of range: ${v.value.seconds}`);
                }
                return;
            }
            case "duration":
                if (typeof v.nanoseconds !== "bigint" ||
                    v.nanoseconds < S64_MIN$1 ||
                    v.nanoseconds > S64_MAX$1) {
                    throw new SchemaEncodeError(`duration nanoseconds out of range: ${v.nanoseconds}`);
                }
                return;
            case "secret": {
                if (!isGuestSecretHandle(v.handle)) {
                    throw new SchemaEncodeError("secret value contains an invalid secret handle");
                }
                assertCapabilityReady(v.handle);
                const raw = peekGuestSecretHandle(SECRET_INTERNAL, v.handle);
                if (raw === undefined) {
                    throw new SchemaEncodeError("secret handle was already transferred; an owned secret can only be sent once");
                }
                if (seen.has(raw)) {
                    throw new SchemaEncodeError("the same secret handle appeared more than once in one value tree");
                }
                seen.add(raw);
                return;
            }
            case "quota-token": {
                if (!isGuestQuotaTokenHandle(v.handle)) {
                    throw new SchemaEncodeError("quota-token value contains an invalid quota-token handle");
                }
                assertCapabilityReady(v.handle);
                // Peek the underlying owned resource without consuming it, so two
                // distinct holders wrapping the same raw resource are also rejected, not
                // only the same holder used twice.
                const raw = peekGuestQuotaTokenHandle(QUOTA_INTERNAL, v.handle);
                if (raw === undefined) {
                    throw new SchemaEncodeError("quota-token handle was already transferred; an owned quota-token can only be sent once");
                }
                if (seen.has(raw)) {
                    throw new SchemaEncodeError("the same quota-token handle appeared more than once in one value tree");
                }
                seen.add(raw);
                return;
            }
            case "stream": {
                if (!(v.handle instanceof GuestSchemaValueStreamHandle)) {
                    throw new SchemaEncodeError("stream value contains an invalid stream handle");
                }
                assertCapabilityReady(v.handle.ownership(STREAM_INTERNAL));
                const stream = v.handle.peek();
                if (stream === undefined) {
                    throw new SchemaEncodeError("schema value stream was already transferred");
                }
                if (stream.kind === "native" && !allowNativeStreams) {
                    throw new SchemaEncodeError("native schema value streams require asynchronous encoding");
                }
                if (seen.has(stream.value)) {
                    throw new SchemaEncodeError("the same schema value stream appeared more than once");
                }
                seen.add(stream.value);
                return;
            }
            case "permission-card": {
                if (!isGuestPermissionCardHandle(v.handle)) {
                    throw new SchemaEncodeError("permission-card value contains an invalid permission-card handle");
                }
                assertCapabilityReady(v.handle);
                const raw = peekGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, v.handle);
                if (raw === undefined) {
                    throw new SchemaEncodeError("permission-card handle was already transferred; an owned permission-card can only be sent once");
                }
                if (seen.has(raw)) {
                    throw new SchemaEncodeError("the same permission-card handle appeared more than once in one value tree");
                }
                seen.add(raw);
                return;
            }
            case "record":
                assertDenseModelArray(v.fields, "record fields");
                v.fields.forEach(visit);
                return;
            case "variant":
                checkIntRange("u32", v.caseIndex);
                if (v.payload !== undefined)
                    visit(v.payload);
                return;
            case "enum":
                checkIntRange("u32", v.caseIndex);
                return;
            case "flags":
                assertDenseModelArray(v.flags, "flags");
                if (v.flags.some((flag) => typeof flag !== "boolean")) {
                    throw new SchemaEncodeError("flags value must be a boolean array");
                }
                return;
            case "tuple":
            case "list":
            case "fixed-list":
                assertDenseModelArray(v.elements, `${v.tag} elements`);
                v.elements.forEach(visit);
                return;
            case "map":
                assertDenseModelArray(v.entries, "map entries");
                v.entries.forEach((e) => {
                    visit(e.key);
                    visit(e.value);
                });
                return;
            case "option":
                if (v.value !== undefined)
                    visit(v.value);
                return;
            case "result":
                if (v.result.value !== undefined)
                    visit(v.result.value);
                return;
            case "union":
                if (typeof v.unionTag !== "string") {
                    throw new SchemaEncodeError(`union tag must be a string: ${String(v.unionTag)}`);
                }
                visit(v.body);
                return;
            case "text":
                if (typeof v.text !== "string" ||
                    (v.language !== undefined && typeof v.language !== "string")) {
                    throw new SchemaEncodeError("text value must contain string text and language values");
                }
                return;
            case "binary":
                if (!(v.bytes instanceof Uint8Array) ||
                    (v.mimeType !== undefined && typeof v.mimeType !== "string")) {
                    throw new SchemaEncodeError("binary value must contain Uint8Array bytes and a string MIME type");
                }
                return;
            case "quantity":
                if (typeof v.value !== "object" ||
                    v.value === null ||
                    typeof v.value.mantissa !== "bigint" ||
                    v.value.mantissa < S64_MIN$1 ||
                    v.value.mantissa > S64_MAX$1 ||
                    !Number.isInteger(v.value.scale) ||
                    v.value.scale < INT_RANGES.s32[0] ||
                    v.value.scale > INT_RANGES.s32[1] ||
                    typeof v.value.unit !== "string") {
                    throw new SchemaEncodeError("quantity value is not representable as s64, s32, and string");
                }
                return;
            default:
                throw new SchemaEncodeError(`unknown schema value tag '${v.tag}'`);
        }
    };
    visit(value);
}
function schemaValueToWit(value) {
    assertSchemaValueRepresentable(value);
    const valueNodes = [];
    function emit(v) {
        valueNodes.push(emitNode(v));
        return valueNodes.length - 1;
    }
    function emitNode(v) {
        switch (v.tag) {
            case "bool":
                return { tag: "bool-value", val: v.value };
            case "s8":
                return { tag: "s8-value", val: v.value };
            case "s16":
                return { tag: "s16-value", val: v.value };
            case "s32":
                return { tag: "s32-value", val: v.value };
            case "s64":
                return { tag: "s64-value", val: v.value };
            case "u8":
                return { tag: "u8-value", val: v.value };
            case "u16":
                return { tag: "u16-value", val: v.value };
            case "u32":
                return { tag: "u32-value", val: v.value };
            case "u64":
                return { tag: "u64-value", val: v.value };
            case "f32":
                return { tag: "f32-value", val: v.value };
            case "f64":
                return { tag: "f64-value", val: v.value };
            case "char":
                return { tag: "char-value", val: v.value };
            case "string":
                return { tag: "string-value", val: v.value };
            case "record":
                return { tag: "record-value", val: v.fields.map((f) => emit(f)) };
            case "variant":
                return {
                    tag: "variant-value",
                    val: {
                        case_: v.caseIndex,
                        payload: v.payload !== undefined ? emit(v.payload) : undefined,
                    },
                };
            case "enum":
                return { tag: "enum-value", val: v.caseIndex };
            case "flags":
                return { tag: "flags-value", val: v.flags };
            case "tuple":
                return { tag: "tuple-value", val: v.elements.map((e) => emit(e)) };
            case "list":
                return { tag: "list-value", val: v.elements.map((e) => emit(e)) };
            case "fixed-list":
                return { tag: "fixed-list-value", val: v.elements.map((e) => emit(e)) };
            case "map":
                return {
                    tag: "map-value",
                    val: v.entries.map((e) => ({ key: emit(e.key), value: emit(e.value) })),
                };
            case "option":
                return { tag: "option-value", val: v.value !== undefined ? emit(v.value) : undefined };
            case "result":
                return {
                    tag: "result-value",
                    val: v.result.tag === "ok"
                        ? {
                            tag: "ok-value",
                            val: v.result.value !== undefined ? emit(v.result.value) : undefined,
                        }
                        : {
                            tag: "err-value",
                            val: v.result.value !== undefined ? emit(v.result.value) : undefined,
                        },
                };
            case "text":
                return { tag: "text-value", val: { text: v.text, language: v.language } };
            case "binary":
                return { tag: "binary-value", val: { bytes: v.bytes, mimeType: v.mimeType } };
            case "path":
                return { tag: "path-value", val: v.value };
            case "url":
                return { tag: "url-value", val: v.value };
            case "uuid":
                return { tag: "uuid-value", val: v.value };
            case "datetime":
                return { tag: "datetime-value", val: v.value };
            case "duration":
                return { tag: "duration-value", val: { nanoseconds: v.nanoseconds } };
            case "quantity":
                return { tag: "quantity-value-node", val: v.value };
            case "union":
                return { tag: "union-value", val: { tag: v.unionTag, body: emit(v.body) } };
            case "secret": {
                const node = { tag: "secret-value", val: undefined };
                const raw = takeGuestSecretHandleToWire(SECRET_INTERNAL, v.handle, node);
                if (raw === undefined) {
                    throw new SchemaEncodeError("secret handle was already transferred; an owned secret can only be sent once");
                }
                node.val = raw;
                return node;
            }
            case "quota-token": {
                // Move the owned `own<quota-token>` resource out of the take-once cell.
                // The preflight above guarantees the handle is present and unique, so
                // this take always succeeds; the guard is defensive only.
                const node = { tag: "quota-token-handle", val: undefined };
                const raw = takeGuestQuotaTokenHandleToWire(QUOTA_INTERNAL, v.handle, node);
                if (raw === undefined) {
                    throw new SchemaEncodeError("quota-token handle was already transferred; an owned quota-token can only be sent once");
                }
                node.val = raw;
                return node;
            }
            case "stream": {
                const stream = v.handle.take();
                if (stream === undefined) {
                    throw new SchemaEncodeError("schema value stream was already transferred");
                }
                if (stream.kind !== "wrapped") {
                    throw new SchemaEncodeError("native schema value streams require asynchronous encoding");
                }
                return { tag: "stream-value", val: stream.value };
            }
            case "permission-card": {
                const node = { tag: "permission-card-handle", val: undefined };
                const raw = takeGuestPermissionCardHandleToWire(PERMISSION_CARD_INTERNAL, v.handle, node);
                if (raw === undefined) {
                    throw new SchemaEncodeError("permission-card handle was already transferred; an owned permission-card can only be sent once");
                }
                node.val = raw;
                return node;
            }
            default:
                throw new SchemaEncodeError(`unknown schema value tag '${v.tag}'`);
        }
    }
    const root = emit(value);
    return { valueNodes, root };
}
async function schemaValueToWitAsync(value, signal) {
    // This walk is deliberately complete and side-effect free. In particular,
    // do not start wrapping an earlier stream until malformed, consumed, or
    // aliased capabilities anywhere else in the tree have been rejected.
    assertSchemaValueRepresentable(value, true);
    const transaction = {};
    const prepared = [];
    let aborted = false;
    const dispose = (value) => {
        const drop = value[Symbol.dispose];
        try {
            drop?.call(value);
        }
        catch {
            // Continue disposing siblings while preserving the conversion failure.
        }
    };
    const abort = () => {
        if (aborted)
            return;
        aborted = true;
        for (const entry of prepared) {
            entry.proxy.abort();
            entry.original.unreserve(transaction);
            if (entry.wrapped !== undefined)
                dispose(entry.wrapped);
        }
    };
    signal?.addEventListener("abort", abort, { once: true });
    const checkActive = () => {
        if (aborted || signal?.aborted)
            throw new SchemaEncodeError("stream conversion interrupted");
    };
    const streams = [];
    function prepare(current) {
        checkActive();
        switch (current.tag) {
            case "stream": {
                const copy = { ...current };
                streams.push(copy);
                return copy;
            }
            case "record": {
                const fields = [];
                for (const field of current.fields)
                    fields.push(prepare(field));
                return { ...current, fields };
            }
            case "tuple":
            case "list":
            case "fixed-list": {
                const elements = [];
                for (const element of current.elements)
                    elements.push(prepare(element));
                return { ...current, elements };
            }
            case "variant":
                return {
                    ...current,
                    payload: current.payload === undefined ? undefined : prepare(current.payload),
                };
            case "map": {
                const entries = [];
                for (const entry of current.entries) {
                    entries.push({ key: prepare(entry.key), value: prepare(entry.value) });
                }
                return { ...current, entries };
            }
            case "option":
                return {
                    ...current,
                    value: current.value === undefined ? undefined : prepare(current.value),
                };
            case "result":
                return {
                    ...current,
                    result: {
                        ...current.result,
                        value: current.result.value === undefined ? undefined : prepare(current.result.value),
                    },
                };
            case "union":
                return { ...current, body: prepare(current.body) };
            default:
                return current;
        }
    }
    try {
        const ready = prepare(value);
        for (const current of streams) {
            checkActive();
            const stream = current.handle.peek();
            if (stream === undefined) {
                throw new SchemaEncodeError("schema value stream was already transferred");
            }
            if (stream.kind === "wrapped")
                continue;
            current.handle.reserve(transaction);
            const entry = {
                original: current.handle,
                proxy: new PreparedStream(stream.value),
                wrapped: undefined,
            };
            prepared.push(entry);
            checkActive();
            const wrapped = await wrapSchemaValueStream(entry.proxy);
            if (aborted) {
                dispose(wrapped);
                checkActive();
            }
            entry.wrapped = wrapped;
            current.handle = new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
                kind: "wrapped",
                value: wrapped,
            });
        }
        checkActive();
        for (const entry of prepared) {
            if (!entry.proxy.pending || entry.original.peek() === undefined) {
                throw new SchemaEncodeError("stream preparation was closed before handoff");
            }
        }
        const wire = schemaValueToWit(ready);
        for (const entry of prepared) {
            entry.original.take(transaction);
            entry.proxy.commit();
        }
        return wire;
    }
    catch (error) {
        abort();
        throw error;
    }
    finally {
        signal?.removeEventListener("abort", abort);
    }
}
/**
 * Validate a flat value tree by reference without lifting or mutating handles.
 * For reachable nodes, this checks dense carrier payloads and the cross-node
 * invariants and domains that lifting relies on. Across the whole node array it
 * enforces handle/object safety; unreachable nodes otherwise receive shape and
 * handle-safety checks only. If this returns, the lifting walk cannot fail
 * because of carrier structure. Any new lift-time invariant must be mirrored
 * here.
 *
 * This intentionally duplicates the structure of `fromNode` below; the two must
 * be kept in sync (a new node kind must be handled in both, or it will be
 * reported as an unknown tag here).
 */
function preflightWitValueTree(nodes, root) {
    function fail(message) {
        throw new SchemaDecodeError(message);
    }
    function object(value, name) {
        if (typeof value !== "object" || value === null || Array.isArray(value)) {
            return fail(`${name} must be an object`);
        }
        return value;
    }
    function array(value, name) {
        if (!Array.isArray(value))
            return fail(`${name} must be an array`);
        for (let i = 0; i < value.length; i++) {
            if (!(i in value))
                fail(`${name} must be a dense array`);
        }
        return value;
    }
    function string(value, name) {
        if (typeof value !== "string")
            fail(`${name} must be a string`);
    }
    function number(value, name) {
        if (typeof value !== "number")
            fail(`${name} must be a number`);
    }
    function integer(value, name, min, max) {
        if (typeof value !== "number" || !Number.isInteger(value) || value < min || value > max) {
            fail(`${name} must be an integer in [${min}, ${max}]`);
        }
    }
    function rangedBigint(value, name, min, max) {
        if (typeof value !== "bigint" || value < min || value > max) {
            fail(`${name} must be a bigint in [${min}, ${max}]`);
        }
    }
    function index(value, name) {
        integer(value, name, 0, 0xffff_ffff);
        return value;
    }
    function optionalString(value, name) {
        if (value !== undefined)
            string(value, name);
    }
    const onPath = new Uint8Array(nodes.length);
    const reached = new Uint8Array(nodes.length);
    const secretReached = new Set();
    const streamReached = new Set();
    // Indices of owned quota-token and permission-card nodes already reached. An
    // owned handle is affine, so reaching one twice is rejected.
    const ownedHandleReached = new Set();
    // Identities of the underlying owned resources already seen, so two *distinct*
    // handle nodes that somehow carry the same raw resource are rejected too, not
    // only the same node reached twice.
    const seenRaw = new Set();
    function walk(idx) {
        index(idx, "value node index");
        if (idx >= nodes.length) {
            throw new SchemaDecodeError(`value node index out of range: ${idx} (nodes: ${nodes.length})`);
        }
        if (onPath[idx] === 1) {
            throw new SchemaDecodeError(`cyclic value node reference at index ${idx}`);
        }
        onPath[idx] = 1;
        reached[idx] = 1;
        walkNode(idx, nodes[idx]);
        onPath[idx] = 0;
    }
    function walkNode(idx, n) {
        if (typeof n !== "object" || n === null)
            fail(`value node at index ${idx} must be an object`);
        switch (n.tag) {
            case "bool-value":
                if (typeof n.val !== "boolean")
                    fail("bool-value.val must be a boolean");
                return;
            case "s8-value":
                integer(n.val, "s8-value.val", -128, 0x7f);
                return;
            case "s16-value":
                integer(n.val, "s16-value.val", -32768, 0x7fff);
                return;
            case "s32-value":
                integer(n.val, "s32-value.val", -2147483648, 0x7fff_ffff);
                return;
            case "s64-value":
                rangedBigint(n.val, "s64-value.val", -(1n << 63n), (1n << 63n) - 1n);
                return;
            case "u8-value":
                integer(n.val, "u8-value.val", 0, 0xff);
                return;
            case "u16-value":
                integer(n.val, "u16-value.val", 0, 0xffff);
                return;
            case "u32-value":
                integer(n.val, "u32-value.val", 0, 0xffff_ffff);
                return;
            case "u64-value":
                rangedBigint(n.val, "u64-value.val", 0n, (1n << 64n) - 1n);
                return;
            case "f32-value":
            case "f64-value":
                number(n.val, `${n.tag}.val`);
                return;
            case "char-value":
                string(n.val, "char-value.val");
                if ([...n.val].length !== 1 ||
                    (n.val.length === 1 && n.val.charCodeAt(0) >= 0xd800 && n.val.charCodeAt(0) <= 0xdfff)) {
                    fail("char-value.val must contain one Unicode scalar");
                }
                return;
            case "string-value":
            case "path-value":
            case "url-value":
                string(n.val, `${n.tag}.val`);
                return;
            case "uuid-value":
                rangedBigint(n.val.highBits, "uuid-value.val.highBits", 0n, (1n << 64n) - 1n);
                rangedBigint(n.val.lowBits, "uuid-value.val.lowBits", 0n, (1n << 64n) - 1n);
                return;
            case "enum-value":
                integer(n.val, "enum-value.val", 0, 0xffff_ffff);
                return;
            case "flags-value":
                array(n.val, "flags-value.val").forEach((flag) => {
                    if (typeof flag !== "boolean")
                        fail("flags-value.val entries must be booleans");
                });
                return;
            case "text-value": {
                const val = object(n.val, "text-value.val");
                string(val.text, "text-value.val.text");
                optionalString(val.language, "text-value.val.language");
                return;
            }
            case "binary-value": {
                const val = object(n.val, "binary-value.val");
                if (!(val.bytes instanceof Uint8Array))
                    fail("binary-value.val.bytes must be a Uint8Array");
                optionalString(val.mimeType, "binary-value.val.mimeType");
                return;
            }
            case "datetime-value": {
                const val = object(n.val, "datetime-value.val");
                rangedBigint(val.seconds, "datetime-value.val.seconds", -(1n << 63n), (1n << 63n) - 1n);
                integer(val.nanoseconds, "datetime-value.val.nanoseconds", 0, 999_999_999);
                return;
            }
            case "duration-value": {
                const val = object(n.val, "duration-value.val");
                rangedBigint(val.nanoseconds, "duration-value.val.nanoseconds", -(1n << 63n), (1n << 63n) - 1n);
                return;
            }
            case "quantity-value-node": {
                const val = object(n.val, "quantity-value-node.val");
                rangedBigint(val.mantissa, "quantity-value-node.val.mantissa", -(1n << 63n), (1n << 63n) - 1n);
                integer(val.scale, "quantity-value-node.val.scale", -2147483648, 0x7fff_ffff);
                string(val.unit, "quantity-value-node.val.unit");
                return;
            }
            case "secret-value": {
                if (secretReached.has(idx)) {
                    throw new SchemaDecodeError("secret handle referenced more than once");
                }
                const raw = n.val;
                if (raw === undefined) {
                    throw new SchemaDecodeError("secret handle was already transferred");
                }
                if (seenRaw.has(raw)) {
                    throw new SchemaDecodeError("the same secret resource appeared more than once");
                }
                try {
                    assertGuestSecretHandleCanLiftFromWire(SECRET_INTERNAL, raw, n);
                }
                catch (error) {
                    throw new SchemaDecodeError(error instanceof Error ? error.message : String(error));
                }
                seenRaw.add(raw);
                secretReached.add(idx);
                return;
            }
            case "record-value":
                array(n.val, "record-value.val").forEach((child, i) => walk(index(child, `record-value.val[${i}]`)));
                return;
            case "variant-value": {
                const val = object(n.val, "variant-value.val");
                integer(val.case_, "variant-value.val.case_", 0, 0xffff_ffff);
                if (val.payload !== undefined)
                    walk(index(val.payload, "variant-value.val.payload"));
                return;
            }
            case "tuple-value":
            case "list-value":
            case "fixed-list-value":
                array(n.val, `${n.tag}.val`).forEach((child, i) => walk(index(child, `${n.tag}.val[${i}]`)));
                return;
            case "map-value":
                array(n.val, "map-value.val").forEach((entry, i) => {
                    const e = object(entry, `map-value.val[${i}]`);
                    walk(index(e.key, `map-value.val[${i}].key`));
                    walk(index(e.value, `map-value.val[${i}].value`));
                });
                return;
            case "option-value":
                if (n.val !== undefined)
                    walk(index(n.val, "option-value.val"));
                return;
            case "result-value": {
                const r = object(n.val, "result-value.val");
                switch (r.tag) {
                    case "ok-value":
                    case "err-value":
                        if (r.val !== undefined)
                            walk(index(r.val, `result-value.val.${r.tag}`));
                        return;
                    default:
                        throw new SchemaDecodeError(`unknown result value payload tag '${r.tag}'`);
                }
            }
            case "union-value": {
                const val = object(n.val, "union-value.val");
                string(val.tag, "union-value.val.tag");
                walk(index(val.body, "union-value.val.body"));
                return;
            }
            case "quota-token-handle": {
                if (ownedHandleReached.has(idx)) {
                    throw new SchemaDecodeError("quota-token handle referenced more than once");
                }
                // A handle node whose owned resource was already taken out (`val`
                // cleared) cannot be lifted; reject it here so the lifting walk that
                // runs after a successful preflight never throws at a quota node.
                const raw = n.val;
                if (raw === undefined) {
                    throw new SchemaDecodeError("quota-token handle was already transferred");
                }
                if (seenRaw.has(raw)) {
                    throw new SchemaDecodeError("the same quota-token resource appeared more than once");
                }
                try {
                    assertGuestQuotaTokenHandleCanLiftFromWire(QUOTA_INTERNAL, raw, n);
                }
                catch (error) {
                    throw new SchemaDecodeError(error instanceof Error ? error.message : String(error));
                }
                seenRaw.add(raw);
                ownedHandleReached.add(idx);
                return;
            }
            case "permission-card-handle": {
                if (ownedHandleReached.has(idx)) {
                    throw new SchemaDecodeError("permission-card handle referenced more than once");
                }
                const raw = n.val;
                if (raw === undefined) {
                    throw new SchemaDecodeError("permission-card handle was already transferred");
                }
                if (seenRaw.has(raw)) {
                    throw new SchemaDecodeError("the same permission-card resource appeared more than once");
                }
                try {
                    assertGuestPermissionCardHandleCanLiftFromWire(PERMISSION_CARD_INTERNAL, raw, n);
                }
                catch (error) {
                    throw new SchemaDecodeError(error instanceof Error ? error.message : String(error));
                }
                seenRaw.add(raw);
                ownedHandleReached.add(idx);
                return;
            }
            case "stream-value": {
                if (streamReached.has(idx)) {
                    throw new SchemaDecodeError("schema value stream referenced more than once");
                }
                const raw = n.val;
                if (raw === undefined) {
                    throw new SchemaDecodeError("schema value stream was already transferred");
                }
                if (seenRaw.has(raw)) {
                    throw new SchemaDecodeError("the same schema value stream appeared more than once");
                }
                seenRaw.add(raw);
                streamReached.add(idx);
                return;
            }
            default:
                throw new SchemaDecodeError(`unknown schema value node tag '${n.tag}'`);
        }
    }
    walk(root);
    const trailing = reached.findIndex((entry) => entry === 0);
    if (trailing !== -1)
        throw new SchemaDecodeError(`value node ${trailing} is unreachable from the root`);
    // Every owned handle must be reachable from the root exactly once; an
    // unreferenced one makes the tree malformed.
    for (let i = 0; i < nodes.length; i++) {
        const node = nodes[i];
        if (typeof node !== "object" || node === null || Array.isArray(node)) {
            throw new SchemaDecodeError(`value node at index ${i} must be an object`);
        }
        if (node.tag === "secret-value" && !secretReached.has(i)) {
            throw new SchemaDecodeError(`secret handle not referenced from the root: ${i}`);
        }
        if (node.tag === "quota-token-handle" && !ownedHandleReached.has(i)) {
            throw new SchemaDecodeError(`quota-token handle not referenced from the root: ${i}`);
        }
        if (node.tag === "stream-value" && !streamReached.has(i)) {
            throw new SchemaDecodeError(`schema value stream not referenced from the root: ${i}`);
        }
        if (node.tag === "permission-card-handle" && !ownedHandleReached.has(i)) {
            throw new SchemaDecodeError(`permission-card handle not referenced from the root: ${i}`);
        }
    }
}
function schemaValueFromWit(wit, transaction) {
    const nodes = wit.valueNodes;
    preflightWitValueTree(nodes, wit.root);
    const journal = transaction ?? new CapabilityTransaction();
    // Cycle guard tracking the nodes currently on the DFS path: `1` = on path,
    // `0` = off path. A flat `Uint8Array` indexed by node is much cheaper than a
    // `Set<number>` (no per-node hashing for has/add/delete) and lets us drop the
    // per-recursion `try/finally`. Semantics are identical to the previous Set:
    // a back-edge to a node still on the current path is a cycle, while a node
    // shared across sibling branches (a DAG, not a cycle) is re-decoded. On any
    // thrown error the whole decode is aborted and this local array is discarded,
    // so leaving stale `1`s during unwinding is harmless.
    const onPath = new Uint8Array(nodes.length);
    function fromIdx(idx) {
        if (idx < 0 || idx >= nodes.length) {
            throw new SchemaDecodeError(`value node index out of range: ${idx} (nodes: ${nodes.length})`);
        }
        if (onPath[idx] === 1) {
            throw new SchemaDecodeError(`cyclic value node reference at index ${idx}`);
        }
        onPath[idx] = 1;
        const result = fromNode(nodes[idx]);
        onPath[idx] = 0;
        return result;
    }
    function fromNode(n) {
        switch (n.tag) {
            case "bool-value":
                return { tag: "bool", value: n.val };
            case "s8-value":
                return { tag: "s8", value: n.val };
            case "s16-value":
                return { tag: "s16", value: n.val };
            case "s32-value":
                return { tag: "s32", value: n.val };
            case "s64-value":
                return { tag: "s64", value: n.val };
            case "u8-value":
                return { tag: "u8", value: n.val };
            case "u16-value":
                return { tag: "u16", value: n.val };
            case "u32-value":
                return { tag: "u32", value: n.val };
            case "u64-value":
                return { tag: "u64", value: n.val };
            case "f32-value":
                return { tag: "f32", value: n.val };
            case "f64-value":
                return { tag: "f64", value: n.val };
            case "char-value":
                return { tag: "char", value: n.val };
            case "string-value":
                return { tag: "string", value: n.val };
            case "record-value":
                return { tag: "record", fields: n.val.map((i) => fromIdx(i)) };
            case "variant-value":
                return {
                    tag: "variant",
                    caseIndex: n.val.case_,
                    payload: n.val.payload !== undefined ? fromIdx(n.val.payload) : undefined,
                };
            case "enum-value":
                return { tag: "enum", caseIndex: n.val };
            case "flags-value":
                return { tag: "flags", flags: n.val };
            case "tuple-value":
                return { tag: "tuple", elements: n.val.map((i) => fromIdx(i)) };
            case "list-value":
                return { tag: "list", elements: n.val.map((i) => fromIdx(i)) };
            case "fixed-list-value":
                return { tag: "fixed-list", elements: n.val.map((i) => fromIdx(i)) };
            case "map-value":
                return {
                    tag: "map",
                    entries: n.val.map((e) => ({ key: fromIdx(e.key), value: fromIdx(e.value) })),
                };
            case "option-value":
                return { tag: "option", value: n.val !== undefined ? fromIdx(n.val) : undefined };
            case "result-value": {
                const r = n.val;
                switch (r.tag) {
                    case "ok-value":
                        return {
                            tag: "result",
                            result: { tag: "ok", value: r.val !== undefined ? fromIdx(r.val) : undefined },
                        };
                    case "err-value":
                        return {
                            tag: "result",
                            result: { tag: "err", value: r.val !== undefined ? fromIdx(r.val) : undefined },
                        };
                    default:
                        throw new SchemaDecodeError(`unknown result value payload tag '${r.tag}'`);
                }
            }
            case "text-value":
                return { tag: "text", text: n.val.text, language: n.val.language };
            case "binary-value":
                return { tag: "binary", bytes: n.val.bytes, mimeType: n.val.mimeType };
            case "path-value":
                return { tag: "path", value: n.val };
            case "url-value":
                return { tag: "url", value: n.val };
            case "uuid-value":
                return { tag: "uuid", value: n.val };
            case "datetime-value":
                if (typeof n.val.seconds !== "bigint" ||
                    !Number.isInteger(n.val.nanoseconds) ||
                    n.val.nanoseconds < 0 ||
                    n.val.nanoseconds >= 1_000_000_000) {
                    throw new SchemaDecodeError("datetime-value requires bigint seconds and integer nanoseconds in [0, 1000000000)");
                }
                return { tag: "datetime", value: n.val };
            case "duration-value":
                return { tag: "duration", nanoseconds: n.val.nanoseconds };
            case "quantity-value-node":
                return { tag: "quantity", value: n.val };
            case "union-value":
                return { tag: "union", unionTag: n.val.tag, body: fromIdx(n.val.body) };
            case "secret-value": {
                const raw = n.val;
                if (raw === undefined) {
                    throw new SchemaDecodeError("secret handle referenced more than once");
                }
                n.val = undefined;
                let handle;
                try {
                    handle = liftGuestSecretHandleFromWire(SECRET_INTERNAL, raw, n);
                }
                catch (error) {
                    n.val = raw;
                    throw error;
                }
                journal.lock(handle);
                journal.register(() => {
                    n.val = takeGuestSecretHandleToWire(SECRET_INTERNAL, handle, n);
                });
                return { tag: "secret", handle };
            }
            case "quota-token-handle": {
                // Lift the owned `own<quota-token>` resource into an opaque take-once
                // handle. Take-once on the wire node as well, so a malformed tree that
                // references the same handle node twice cannot wrap one owned resource
                // into two handles. `preflightWitValueTree` already validated the whole
                // tree, so this lift cannot be undone by a later failure.
                const raw = n.val;
                if (raw === undefined) {
                    throw new SchemaDecodeError("quota-token handle referenced more than once");
                }
                n.val = undefined;
                const handle = liftGuestQuotaTokenHandleFromWire(QUOTA_INTERNAL, raw, n);
                journal.lock(handle);
                journal.register(() => {
                    n.val = takeGuestQuotaTokenHandleToWire(QUOTA_INTERNAL, handle, n);
                });
                return { tag: "quota-token", handle };
            }
            case "stream-value": {
                const raw = n.val;
                if (raw === undefined) {
                    throw new SchemaDecodeError("schema value stream referenced more than once");
                }
                n.val = undefined;
                const handle = new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
                    kind: "wrapped",
                    value: raw,
                });
                journal.lock(handle.ownership(STREAM_INTERNAL));
                journal.register(() => {
                    n.val = handle.take()?.value;
                });
                return { tag: "stream", handle };
            }
            case "permission-card-handle": {
                const raw = n.val;
                if (raw === undefined) {
                    throw new SchemaDecodeError("permission-card handle referenced more than once");
                }
                n.val = undefined;
                const handle = liftGuestPermissionCardHandleFromWire(PERMISSION_CARD_INTERNAL, raw, n);
                journal.lock(handle);
                journal.register(() => {
                    n.val = takeGuestPermissionCardHandleToWire(PERMISSION_CARD_INTERNAL, handle, n);
                });
                return { tag: "permission-card", handle };
            }
            default:
                throw new SchemaDecodeError(`unknown schema value node tag '${n.tag}'`);
        }
    }
    let result;
    try {
        result = fromIdx(wit.root);
    }
    catch (e) {
        journal.rollback();
        throw e;
    }
    if (transaction === undefined)
        journal.commit();
    return result;
}
/**
 * Clear every owned quota-token, permission-card, or schema-value-stream handle still present in
 * `nodes` (i.e. not moved out during decode) and return the index of the first
 * one found, or `undefined` if none remained.
 */
function drainUnconsumedQuotaAndPermissionCardHandles(nodes) {
    let first;
    for (let i = 0; i < nodes.length; i++) {
        const node = nodes[i];
        if (typeof node !== "object" || node === null || Array.isArray(node))
            continue;
        if ((node.tag === "quota-token-handle" ||
            node.tag === "permission-card-handle" ||
            node.tag === "stream-value") &&
            node.val !== undefined) {
            if (first === undefined)
                first = i;
            if (node.tag === "quota-token-handle") {
                abandonGuestQuotaTokenWireHandle(QUOTA_INTERNAL, node.val, node);
            }
            else if (node.tag === "permission-card-handle") {
                abandonGuestPermissionCardWireHandle(PERMISSION_CARD_INTERNAL, node.val, node);
            }
            try {
                ;
                node.val = undefined;
            }
            catch {
                // The ownership ledger still marks the raw handle transferred even if
                // a malformed non-writable carrier prevents clearing its last JS reference.
            }
        }
    }
    return first;
}
/**
 * Give every permission card, secret, and quota token that an encoder moved into `tree` back to
 * its handle, for a tree that never reached the host. A resource whose handle was released while
 * it was on the wire is dropped instead. Resources without a known origin stay in the tree.
 *
 * Streams are not restored: the encoder has already committed a moved stream to its wire
 * wrapper, so a stream in an unsent tree stays moved. Its wire wrapper is dropped. Call this only
 * for trees that the SDK encoded.
 */
function restoreEncodedCapabilities(tree) {
    for (const node of tree.valueNodes) {
        if (typeof node !== "object" || node === null)
            continue;
        const carrier = node;
        if (carrier.val === undefined)
            continue;
        let result;
        switch (carrier.tag) {
            case "permission-card-handle":
                result = restoreGuestPermissionCardHandleFromWire(PERMISSION_CARD_INTERNAL, carrier.val, node);
                break;
            case "secret-value":
                result = restoreGuestSecretHandleFromWire(SECRET_INTERNAL, carrier.val, node);
                break;
            case "quota-token-handle":
                result = restoreGuestQuotaTokenHandleFromWire(QUOTA_INTERNAL, carrier.val, node);
                break;
            case "stream-value":
                result = "released";
                break;
            default:
                continue;
        }
        if (result === "unknown")
            continue;
        if (result === "released") {
            const drop = carrier.val[Symbol.dispose];
            try {
                drop?.call(carrier.val);
            }
            catch {
                // A failed drop must not stop the remaining restores.
            }
        }
        carrier.val = undefined;
    }
}
/**
 * Return `encoding` unchanged, and give its moved capabilities back when `signal` aborts before
 * the awaiting fiber resumes with the tree. Register this before the caller awaits `encoding`.
 */
function restoreEncodedCapabilitiesOnAbort(signal, encoding) {
    encoding.then((tree) => {
        if (signal.aborted)
            restoreEncodedCapabilities(tree);
    }, () => { });
    return encoding;
}
// ============================================================
// Typed schema value
// ============================================================
function typedSchemaValueToWit(tv) {
    return { graph: schemaGraphToWit(tv.graph), value: schemaValueToWit(tv.value) };
}
function typedSchemaValueFromWit(wit) {
    let graph;
    try {
        graph = schemaGraphFromWit(wit.graph);
    }
    catch (error) {
        drainUnconsumedQuotaAndPermissionCardHandles(wit.value.valueNodes);
        throw error;
    }
    return { graph, value: schemaValueFromWit(wit.value) };
}

/**
 * Convert host invocation metadata to the SDK form. The invocation ID combines the agent ID text
 * with the UUID of the invocation; the owner case of the host ID is not used.
 */
const invocationMetadataFromHost = (metadata) => Object.freeze({
    agentId: metadata.agentId,
    invocationId: makeInvocationId(metadata.agentId, uuidToString(metadata.invocationId.id)),
});
const RPC_TAGS = new Set([
    "protocol-error",
    "denied",
    "not-found",
    "remote-internal-error",
    "remote-agent-error",
]);
const extractRpcError = (error) => {
    const candidates = [error];
    if (error instanceof Error)
        candidates.push(error.payload, error.cause);
    return candidates.find((value) => {
        if (value === null || typeof value !== "object")
            return false;
        const tag = value.tag;
        return typeof tag === "string" && RPC_TAGS.has(tag);
    });
};
const wrapHostThrow = (error) => ({
    _tag: "RpcCallError",
    cause: extractRpcError(error) ?? {
        tag: "protocol-error",
        val: error instanceof Error ? error.message : String(error),
    },
});
const bestEffort = (operation) => {
    try {
        operation();
    }
    catch {
        // Host resource cleanup must not replace the operation's result.
    }
};
const scopeCardUnavailable = (reason) => ({
    _tag: "ScopeCardUnavailableError",
    reason,
});
/** Borrow a scope card for one awaited call. A released or foreign card fails the call. */
const borrowScopeCard = (card) => Effect.suspend(() => {
    if (card === undefined)
        return Effect.succeed(undefined);
    if (!isScopeCard(card))
        return Effect.fail(scopeCardUnavailable("the value is not a scope card"));
    const raw = peekScopeCard(card);
    return raw === undefined
        ? Effect.fail(scopeCardUnavailable("the scope card was released"))
        : Effect.succeed(raw);
});
/**
 * Run one awaited invocation. The scope card is checked before `input` runs, and again when the
 * host is called. When the call fails or is interrupted before the host receives the encoded
 * input, the capabilities that the encoder moved into it go back to their handles (see
 * `restoreEncodedCapabilities`). Set `restoreUnsent` to `false` for a tree that the caller built:
 * it then stays as the caller supplied it.
 *
 * Once `asyncInvokeAndAwait` is called, nothing is restored, even when it throws: the host may
 * already own the input. A handle without metadata is a rejected call: its future fails with the
 * rejection error.
 */
const awaitInvocation = (rpc, method, input, scopeCard, restoreUnsent = true) => Effect.suspend(() => {
    let unsent;
    return borrowScopeCard(scopeCard).pipe(Effect.andThen(Effect.uninterruptibleMask((restore) => Effect.tap(restore(input), (tree) => Effect.sync(() => {
        unsent = tree;
    })))), Effect.flatMap((tree) => Effect.acquireUseRelease(borrowScopeCard(scopeCard).pipe(Effect.flatMap((raw) => Effect.try({
        try: () => {
            unsent = undefined;
            return rpc.asyncInvokeAndAwait(method, tree, raw);
        },
        catch: wrapHostThrow,
    })), Effect.map((invocation) => ({
        invocation,
        metadata: invocation.metadata && invocationMetadataFromHost(invocation.metadata),
    }))), ({ invocation, metadata }) => Effect.tryPromise({ try: () => invocation.get(), catch: wrapHostThrow }).pipe(Effect.flatMap((result) => metadata === undefined
        ? Effect.fail({
            _tag: "RpcCallError",
            cause: {
                tag: "protocol-error",
                val: `${method}: host completed a rejected invocation`,
            },
        })
        : Effect.succeed({ metadata, result }))), ({ invocation }) => Effect.sync(() => {
        bestEffort(() => invocation.cancel());
        bestEffort(() => invocation.drop());
    }))), Effect.onExit((exit) => restoreUnsent && Exit.isFailure(exit) && unsent !== undefined
        ? Effect.sync(() => restoreEncodedCapabilities(unsent))
        : Effect.void));
});
const cancelOnce = (token) => {
    let consumed = false;
    return Effect.sync(() => {
        if (consumed)
            return;
        consumed = true;
        bestEffort(() => token.cancel());
    });
};
const scheduleCancelableInvocation = (rpc, at, method, input) => Effect.acquireRelease(Effect.try({
    try: () => rpc.scheduleCancelableInvocation(at, method, input),
    catch: wrapHostThrow,
}), ({ token }) => Effect.sync(() => bestEffort(() => token.drop()))).pipe(Effect.map(({ metadata, token }) => ({
    metadata: invocationMetadataFromHost(metadata),
    cancel: cancelOnce(token),
})));

const rawPhantoms = new WeakMap();
const retainRawPhantomId = (identity, phantom) => {
    rawPhantoms.set(identity, phantom);
};
const rawPhantomId = (identity) => {
    if (!rawPhantoms.has(identity)) {
        throw new TypeError("Expected an AgentIdentity created by AgentIdentity.parse or AgentIdentity.make");
    }
    return rawPhantoms.get(identity);
};

/** Identity host edges and remote binding, without public identity wrapping or ownership. */
const decodeAgentId = (host, encoded) => host.parseAgentId(encoded);
const encodeAgentId = (host, typeName, constructorValue, phantom) => host.makeAgentId(typeName, constructorValue, phantom);
const connectIdentity = (identity, entries, identityError) => Effect.gen(function* () {
    const phantom = yield* Effect.try({
        try: () => rawPhantomId(identity),
        catch: identityError,
    });
    const host = yield* RpcClient;
    return yield* host
        .connect(identity.typeName, identity.constructorValue, phantom, entries)
        .pipe(Effect.mapError((error) => wrapHostThrow(error.cause)));
});

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
const U32_MAX = 2 ** 32 - 1;
const S32_MIN = -2147483648;
const S32_MAX = 2 ** 31 - 1;
const S64_MIN = -(2n ** 63n);
const S64_MAX = 2n ** 63n - 1n;
const U64_MAX$2 = 2n ** 64n - 1n;
const I128_MIN = -(2n ** 127n);
const I128_MAX = 2n ** 127n - 1n;
const FLOAT_BITS = new DataView(new ArrayBuffer(8));
/**
 * Validate every definition body followed by the graph root, in deterministic order.
 *
 * Regex restrictions are compiled with the platform's Unicode-mode ECMAScript `RegExp`
 * implementation.
 * Rust validates the same schema fields with the `regex` crate, so engine-specific syntax can
 * be accepted by one SDK and rejected by the other. Cross-SDK schemas should use their common
 * regular-expression subset.
 */
function validateSchemaGraph(graph) {
    const errors = [];
    for (const definition of graph.defs.values()) {
        checkType(graph, definition.body, errors, new Set());
    }
    checkType(graph, graph.root, errors, new Set());
    return errors;
}
function issue(errors, code, message, refId) {
    errors.push({ code, message, ...(refId === undefined ? {} : { refId }) });
}
function checkType(graph, type, errors, onPath) {
    if (onPath.has(type)) {
        issue(errors, "inline-cycle", "schema contains an inline object-reference cycle");
        return;
    }
    onPath.add(type);
    try {
        const body = type.body;
        switch (body.tag) {
            case "ref": {
                const resolved = resolveAlias(graph, type);
                if (resolved.tag === "dangling") {
                    issue(errors, "dangling-ref", `dangling type reference \`${resolved.id}\``, resolved.id);
                }
                else if (resolved.tag === "recursive") {
                    issue(errors, "recursive-alias", `type reference \`${resolved.id}\` forms a reference cycle with no concrete type`, resolved.id);
                }
                break;
            }
            case "record":
                checkDuplicates(body.fields.map((field) => field.name), (name) => issue(errors, "duplicate-field-name", `duplicate field \`${name}\``));
                body.fields.forEach((field) => checkType(graph, field.body, errors, onPath));
                break;
            case "variant":
                if (body.cases.length === 0)
                    issue(errors, "empty-variant", "variant has no cases");
                checkDuplicates(body.cases.map((entry) => entry.name), (name) => issue(errors, "duplicate-variant-case", `duplicate variant case \`${name}\``));
                body.cases.forEach((entry) => {
                    if (entry.payload)
                        checkType(graph, entry.payload, errors, onPath);
                });
                break;
            case "enum":
                if (body.cases.length === 0)
                    issue(errors, "empty-enum", "enum has no cases");
                checkDuplicates(body.cases, (name) => issue(errors, "duplicate-enum-case", `duplicate enum case \`${name}\``));
                break;
            case "flags":
                if (body.names.length === 0)
                    issue(errors, "empty-flags", "flags has no entries");
                checkDuplicates(body.names, (name) => issue(errors, "duplicate-flag-name", `duplicate flag \`${name}\``));
                break;
            case "tuple":
                body.elements.forEach((element) => checkType(graph, element, errors, onPath));
                break;
            case "list":
                checkType(graph, body.element, errors, onPath);
                break;
            case "fixed-list":
                if (!isU32(body.length) || body.length === 0) {
                    issue(errors, "invalid-fixed-list-length", "fixed-list length must be a positive u32");
                }
                checkType(graph, body.element, errors, onPath);
                break;
            case "map":
                if (classifyMapKey(graph, body.key) !== "primitive") {
                    issue(errors, "map-key-not-primitive", "map key must be a primitive type");
                }
                checkType(graph, body.key, errors, onPath);
                checkType(graph, body.value, errors, onPath);
                break;
            case "option":
                if (isNullable(graph, body.element, new Set())) {
                    issue(errors, "nullable-nesting", `option<${describeNullable(body.element)}> is invalid because the inner type is also nullable`);
                }
                checkType(graph, body.element, errors, onPath);
                break;
            case "result":
                if (body.ok)
                    checkType(graph, body.ok, errors, onPath);
                if (body.err)
                    checkType(graph, body.err, errors, onPath);
                break;
            case "text":
                checkTextRestrictions(body.restrictions, errors);
                break;
            case "binary":
                checkBinaryRestrictions(body.restrictions, errors);
                break;
            case "quantity":
                checkQuantity(body.spec, errors);
                break;
            case "union":
                checkUnion(graph, body.branches, errors, onPath);
                break;
            case "secret":
                checkType(graph, body.inner, errors, onPath);
                break;
            case "future":
            case "stream":
                if (body.element)
                    checkType(graph, body.element, errors, onPath);
                break;
            case "s8":
            case "s16":
            case "s32":
            case "s64":
            case "u8":
            case "u16":
            case "u32":
            case "u64":
            case "f32":
            case "f64":
                checkNumericRestrictions(body.tag, body.restrictions, errors);
                break;
            case "bool":
            case "char":
            case "string":
            case "path":
            case "url":
            case "uuid":
            case "datetime":
            case "duration":
            case "quota-token":
            case "permission-card":
                break;
        }
    }
    finally {
        onPath.delete(type);
    }
}
function checkDuplicates(values, onDuplicate) {
    const seen = new Set();
    for (const value of values) {
        if (seen.has(value))
            onDuplicate(value);
        else
            seen.add(value);
    }
}
function resolveAlias(graph, type) {
    let current = type;
    const visited = new Set();
    while (current.body.tag === "ref") {
        const id = current.body.id;
        if (visited.has(id))
            return { tag: "recursive", id };
        visited.add(id);
        const definition = graph.defs.get(id);
        if (!definition)
            return { tag: "dangling", id };
        current = definition.body;
    }
    return { tag: "concrete", type: current };
}
function classifyMapKey(graph, type) {
    const resolved = resolveAlias(graph, type);
    if (resolved.tag !== "concrete")
        return "unresolved";
    return isPrimitive(resolved.type) ? "primitive" : "non-primitive";
}
function isPrimitive(type) {
    return [
        "bool",
        "s8",
        "s16",
        "s32",
        "s64",
        "u8",
        "u16",
        "u32",
        "u64",
        "f32",
        "f64",
        "char",
        "string",
    ].includes(type.body.tag);
}
function isNullable(graph, type, visited) {
    switch (type.body.tag) {
        case "option":
            return true;
        case "union":
            return type.body.branches.some((branch) => isNullable(graph, branch.body, visited));
        case "ref": {
            const id = type.body.id;
            if (visited.has(id))
                return false;
            visited.add(id);
            const definition = graph.defs.get(id);
            const result = definition ? isNullable(graph, definition.body, visited) : false;
            visited.delete(id);
            return result;
        }
        default:
            return false;
    }
}
function describeNullable(type) {
    switch (type.body.tag) {
        case "option":
            return "option<_>";
        case "union":
            return "union";
        case "ref":
            return `ref \`${type.body.id}\``;
        default:
            return "nullable";
    }
}
function checkNumericRestrictions(tag, restrictions, errors) {
    if (!restrictions)
        return;
    const { min, max, unit } = restrictions;
    const reason = min === undefined && max === undefined && !unit
        ? "numeric restriction set is empty"
        : (checkNumericBound(tag, min) ??
            checkNumericBound(tag, max) ??
            (min && max && compareNumericBounds(min, max) === 1
                ? "numeric min bound is greater than max bound"
                : undefined));
    if (reason)
        issue(errors, "invalid-numeric-restriction", reason);
}
function checkNumericBound(tag, bound) {
    if (!bound)
        return undefined;
    const family = tag.startsWith("s") ? "signed" : tag.startsWith("u") ? "unsigned" : "float-bits";
    if (bound.tag !== family)
        return "numeric bound family does not match the numeric type";
    if (typeof bound.val !== "bigint")
        return "numeric bound is not representable on the WIT wire";
    if (bound.tag === "float-bits") {
        const value = floatFromBits(bound.val);
        if (value === undefined)
            return "numeric float bits do not fit u64";
        if (!Number.isFinite(value))
            return "numeric float bound must be finite";
        if (tag === "f32" && Math.fround(value) !== value) {
            return "f32 numeric bound does not round-trip through f32";
        }
        return undefined;
    }
    const range = numericRange(tag);
    return !range || bound.val < range[0] || bound.val > range[1]
        ? "numeric bound does not fit the numeric type range"
        : undefined;
}
function numericRange(tag) {
    switch (tag) {
        case "s8":
            return [-128n, 127n];
        case "s16":
            return [-32768n, 32767n];
        case "s32":
            return [-(2n ** 31n), 2n ** 31n - 1n];
        case "s64":
            return [S64_MIN, S64_MAX];
        case "u8":
            return [0n, 255n];
        case "u16":
            return [0n, 65535n];
        case "u32":
            return [0n, 2n ** 32n - 1n];
        case "u64":
            return [0n, U64_MAX$2];
        case "f32":
        case "f64":
            return undefined;
    }
}
function compareNumericBounds(left, right) {
    if (left.tag !== right.tag)
        return undefined;
    if (left.tag !== "float-bits") {
        return left.val < right.val ? -1 : left.val > right.val ? 1 : 0;
    }
    const a = floatFromBits(left.val);
    const b = floatFromBits(right.val);
    if (a === undefined || b === undefined || !Number.isFinite(a) || !Number.isFinite(b)) {
        return undefined;
    }
    return a < b ? -1 : a > b ? 1 : 0;
}
function floatFromBits(bits) {
    if (typeof bits !== "bigint" || bits < 0n || bits > U64_MAX$2)
        return undefined;
    FLOAT_BITS.setBigUint64(0, bits);
    return FLOAT_BITS.getFloat64(0);
}
function checkTextRestrictions(restrictions, errors) {
    if (!validU32Range(restrictions.minLength, restrictions.maxLength)) {
        issue(errors, "invalid-text-length-range", "text length bounds must be ordered u32 values");
    }
    if (restrictions.regex !== undefined) {
        try {
            new RegExp(restrictions.regex, "u");
        }
        catch (error) {
            issue(errors, "invalid-text-regex", `text regex \`${restrictions.regex}\` failed to compile: ${errorMessage(error)}`);
        }
    }
}
function checkBinaryRestrictions(restrictions, errors) {
    if (!validU32Range(restrictions.minBytes, restrictions.maxBytes)) {
        issue(errors, "invalid-binary-byte-range", "binary byte bounds must be ordered u32 values");
    }
}
function validU32Range(min, max) {
    return ((min === undefined || isU32(min)) &&
        (max === undefined || isU32(max)) &&
        (min === undefined || max === undefined || min <= max));
}
function isU32(value) {
    return Number.isInteger(value) && value >= 0 && value <= U32_MAX;
}
function isQuantityValueRepresentable(value) {
    return (typeof value === "object" &&
        value !== null &&
        typeof value.mantissa === "bigint" &&
        value.mantissa >= S64_MIN &&
        value.mantissa <= S64_MAX &&
        Number.isInteger(value.scale) &&
        value.scale >= S32_MIN &&
        value.scale <= S32_MAX &&
        typeof value.unit === "string");
}
function quantityLessOrEqual(left, right) {
    if (!isQuantityValueRepresentable(left) || !isQuantityValueRepresentable(right))
        return undefined;
    const commonScale = Math.max(left.scale, right.scale);
    const leftShift = commonScale - left.scale;
    const rightShift = commonScale - right.scale;
    if (leftShift > 38 || rightShift > 38)
        return undefined;
    const leftValue = left.mantissa * 10n ** BigInt(leftShift);
    const rightValue = right.mantissa * 10n ** BigInt(rightShift);
    if (leftValue < I128_MIN ||
        leftValue > I128_MAX ||
        rightValue < I128_MIN ||
        rightValue > I128_MAX) {
        return undefined;
    }
    return leftValue <= rightValue;
}
function checkQuantity(spec, errors) {
    const minValid = spec.min === undefined || isQuantityValueRepresentable(spec.min);
    const maxValid = spec.max === undefined || isQuantityValueRepresentable(spec.max);
    if (!minValid || !maxValid) {
        issue(errors, "invalid-quantity-bound", "quantity bounds require an s64 mantissa and s32 scale");
    }
    if (spec.min && minValid && spec.min.unit !== spec.baseUnit) {
        issue(errors, "quantity-min-unit-mismatch", `quantity min unit mismatch: base \`${spec.baseUnit}\`, min \`${spec.min.unit}\``);
    }
    if (spec.max && maxValid && spec.max.unit !== spec.baseUnit) {
        issue(errors, "quantity-max-unit-mismatch", `quantity max unit mismatch: base \`${spec.baseUnit}\`, max \`${spec.max.unit}\``);
    }
    if (spec.min &&
        spec.max &&
        minValid &&
        maxValid &&
        spec.min.unit === spec.baseUnit &&
        spec.max.unit === spec.baseUnit) {
        const comparison = quantityLessOrEqual(spec.min, spec.max);
        if (comparison === false) {
            issue(errors, "quantity-min-greater-than-max", "quantity min is greater than max");
        }
        else if (comparison === undefined) {
            issue(errors, "quantity-comparison-overflow", `quantity range comparison overflowed in base unit \`${spec.baseUnit}\``);
        }
    }
}
function checkUnion(graph, branches, errors, onPath) {
    if (branches.length === 0)
        issue(errors, "empty-union", "union has no branches");
    checkDuplicates(branches.map((branch) => branch.tag), (tag) => issue(errors, "duplicate-union-tag", `duplicate union branch tag \`${tag}\``));
    for (const branch of branches) {
        checkUnionBranch(graph, branch, errors);
        checkType(graph, branch.body, errors, onPath);
    }
    for (let left = 0; left < branches.length; left += 1) {
        for (let right = left + 1; right < branches.length; right += 1) {
            const classification = classifyDiscriminatorPair(branches[left].discriminator, branches[right].discriminator);
            if (classification.tag === "reject") {
                issue(errors, "union-ambiguous-discriminators", `union branches \`${branches[left].tag}\` and \`${branches[right].tag}\` have overlapping discriminators (${classification.reason})`);
            }
        }
    }
}
function checkUnionBranch(graph, branch, errors) {
    const shape = resolvedShape(graph, branch.body);
    const shapeKnown = shape.tag !== "unresolved";
    const rule = branch.discriminator;
    switch (rule.tag) {
        case "prefix":
        case "suffix":
        case "contains":
            if (shapeKnown && shape.tag !== "string") {
                issue(errors, "union-string-rule-on-non-string-body", `union branch \`${branch.tag}\` uses a string-pattern rule but body is not string-shaped`);
            }
            break;
        case "regex":
            if (shapeKnown && shape.tag !== "string") {
                issue(errors, "union-string-rule-on-non-string-body", `union branch \`${branch.tag}\` uses a string-pattern rule but body is not string-shaped`);
            }
            if (rule.val.length === 0) {
                issue(errors, "invalid-union-regex", `union branch \`${branch.tag}\` regex must be non-empty`);
            }
            else {
                try {
                    new RegExp(rule.val, "u");
                }
                catch (error) {
                    issue(errors, "invalid-union-regex", `union branch \`${branch.tag}\` regex \`${rule.val}\` failed to compile: ${errorMessage(error)}`);
                }
            }
            break;
        case "field-equals":
            if (shape.tag === "record") {
                const field = shape.fields.fields.find((entry) => entry.name === rule.val.fieldName);
                if (!field) {
                    issue(errors, "union-field-rule-missing-field", `union branch \`${branch.tag}\` references missing field \`${rule.val.fieldName}\``);
                }
                else if (rule.val.literal !== undefined) {
                    const fieldShape = resolvedShape(graph, field.body);
                    if (fieldShape.tag !== "string" && fieldShape.tag !== "unresolved") {
                        issue(errors, "union-field-equals-literal-on-non-string-field", `union branch \`${branch.tag}\` compares a literal against non-string field \`${rule.val.fieldName}\``);
                    }
                }
            }
            else if (shapeKnown) {
                issue(errors, "union-field-rule-on-non-record-body", `union branch \`${branch.tag}\` uses a field rule but body is not record-shaped`);
            }
            break;
        case "field-absent":
            if (shape.tag === "record") {
                if (shape.fields.fields.some((entry) => entry.name === rule.val)) {
                    issue(errors, "union-unsatisfiable-field-absent", `union branch \`${branch.tag}\` requires declared field \`${rule.val}\` to be absent`);
                }
            }
            else if (shapeKnown) {
                issue(errors, "union-field-rule-on-non-record-body", `union branch \`${branch.tag}\` uses a field rule but body is not record-shaped`);
            }
            break;
    }
}
function resolvedShape(graph, type) {
    const resolved = resolveAlias(graph, type);
    if (resolved.tag !== "concrete")
        return { tag: "unresolved" };
    switch (resolved.type.body.tag) {
        case "string":
        case "url":
        case "path":
            return { tag: "string" };
        case "record":
            return { tag: "record", fields: resolved.type.body };
        default:
            return { tag: "other" };
    }
}
function classifyDiscriminatorPair(left, right) {
    if (left.tag === "prefix" && right.tag === "prefix") {
        return left.val.startsWith(right.val) || right.val.startsWith(left.val)
            ? { tag: "reject", reason: `prefix \`${left.val}\` and prefix \`${right.val}\` overlap` }
            : { tag: "disjoint" };
    }
    if (left.tag === "suffix" && right.tag === "suffix") {
        return left.val.endsWith(right.val) || right.val.endsWith(left.val)
            ? { tag: "reject", reason: `suffix \`${left.val}\` and suffix \`${right.val}\` overlap` }
            : { tag: "disjoint" };
    }
    if ((left.tag === "prefix" && right.tag === "suffix") ||
        (left.tag === "suffix" && right.tag === "prefix")) {
        const prefix = left.tag === "prefix" ? left.val : right.val;
        const suffix = left.tag === "suffix" ? left.val : right.val;
        return { tag: "reject", reason: `prefix \`${prefix}\` and suffix \`${suffix}\` overlap` };
    }
    if ((left.tag === "prefix" && right.tag === "contains") ||
        (left.tag === "contains" && right.tag === "prefix")) {
        const prefix = left.tag === "prefix" ? left.val : right.val;
        const contains = left.tag === "contains" ? left.val : right.val;
        return { tag: "reject", reason: `prefix \`${prefix}\` and contains \`${contains}\` overlap` };
    }
    if ((left.tag === "suffix" && right.tag === "contains") ||
        (left.tag === "contains" && right.tag === "suffix")) {
        const suffix = left.tag === "suffix" ? left.val : right.val;
        const contains = left.tag === "contains" ? left.val : right.val;
        return { tag: "reject", reason: `suffix \`${suffix}\` and contains \`${contains}\` overlap` };
    }
    if (left.tag === "contains" && right.tag === "contains") {
        return {
            tag: "reject",
            reason: `contains \`${left.val}\` and contains \`${right.val}\` overlap`,
        };
    }
    if (left.tag === "regex" && right.tag === "regex") {
        return left.val === right.val
            ? { tag: "reject", reason: `both branches share regex \`${left.val}\`` }
            : { tag: "indeterminate" };
    }
    const stringPattern = (rule) => rule.tag === "prefix" || rule.tag === "suffix" || rule.tag === "contains";
    if ((left.tag === "regex" && stringPattern(right)) ||
        (right.tag === "regex" && stringPattern(left))) {
        return { tag: "indeterminate" };
    }
    const stringShaped = (rule) => stringPattern(rule) || rule.tag === "regex";
    const recordShaped = (rule) => rule.tag === "field-equals" || rule.tag === "field-absent";
    if ((stringShaped(left) && recordShaped(right)) || (recordShaped(left) && stringShaped(right))) {
        return { tag: "disjoint" };
    }
    if (left.tag === "field-equals" && right.tag === "field-equals") {
        if (left.val.fieldName !== right.val.fieldName) {
            return {
                tag: "reject",
                reason: `field-equals on \`${left.val.fieldName}\` and \`${right.val.fieldName}\` can match the same object`,
            };
        }
        if (left.val.literal === undefined || right.val.literal === undefined) {
            return {
                tag: "reject",
                reason: `field-equals on \`${left.val.fieldName}\` without literal overlaps another field-equals on the same field`,
            };
        }
        return left.val.literal === right.val.literal
            ? {
                tag: "reject",
                reason: `two field-equals on \`${left.val.fieldName}\` share literal \`${left.val.literal}\``,
            }
            : { tag: "disjoint" };
    }
    if (left.tag === "field-absent" && right.tag === "field-absent") {
        return {
            tag: "reject",
            reason: `field-absent on \`${left.val}\` and \`${right.val}\` can match the same object`,
        };
    }
    if ((left.tag === "field-equals" && right.tag === "field-absent") ||
        (left.tag === "field-absent" && right.tag === "field-equals")) {
        const fieldEquals = left.tag === "field-equals"
            ? left.val
            : right.val;
        const fieldAbsent = left.tag === "field-absent"
            ? left.val
            : right.val;
        return fieldEquals.fieldName === fieldAbsent
            ? { tag: "disjoint" }
            : {
                tag: "reject",
                reason: `field-equals on \`${fieldEquals.fieldName}\` and field-absent on \`${fieldAbsent}\` can match the same object`,
            };
    }
    return { tag: "indeterminate" };
}
function errorMessage(error) {
    return error instanceof Error ? error.message : String(error);
}

const WIT_QUOTA_TOKEN_ANNOTATION_KEY = "effect-golem/witQuotaToken";
const WIT_QUOTA_TOKEN_RESOURCE_NAME_ANNOTATION_KEY = "effect-golem/witQuotaTokenResourceName";

class QuotaClient extends Context.Service()("effect-golem/host/Quota") {
}
const QuotaLive = Layer.succeed(QuotaClient, QuotaClient.of({
    newToken: QuotaHost.newToken,
    reserve: QuotaHost.reserve,
    commit: QuotaHost.Reservation.commit,
    split: QuotaHost.split,
    merge: QuotaHost.merge,
}));

/**
 * @since 1.5.0
 */
/**
 * Annotation key used by the codec to override the default WIT primitive
 * type for a numeric schema. Values are the `WitTypeNode` primitive tags
 * without the `prim-` / `-type` decorations: `"u8"`, `"u16"`, `"u32"`,
 * `"u64"`, `"s8"`, `"s16"`, `"s32"`, `"s64"`, `"f32"`, `"f64"`.
 *
 * `Schema.Number` defaults to `f64`, `Schema.BigInt` defaults to `s64`.
 * The helpers below pre-apply the right annotation.
 *
 * @since 1.5.0
 * @category utils
 */
const witTypeAnnotationKey = "effect-golem/witType";
const tag = (kind) => ({ [witTypeAnnotationKey]: kind });
/**
 * Annotation key carrying inline numeric min/max/unit restrictions on a numeric
 * pin schema. The codec lowers it to the WIT `numeric-restrictions` payload.
 *
 * @since 1.6.0
 * @category utils
 */
const witNumericRestrictionsKey = "effect-golem/witNumericRestrictions";
/**
 * Integer pin: `Schema.Int` (rejects non-integers/NaN/Infinity) narrowed to the
 * WIT width's inclusive range, then tagged. Enforced on the invocation boundary.
 */
const intPin = (kind, min, max) => Schema.Int.check(Schema.isBetween({ minimum: min, maximum: max })).pipe(Schema.annotate(tag(kind)));
/** BigInt pin: `Schema.BigInt` narrowed to the WIT width's inclusive range, then tagged. */
const bigPin = (kind, min, max) => Schema.BigInt.check(Schema.isGreaterThanOrEqualToBigInt(min), Schema.isLessThanOrEqualToBigInt(max)).pipe(Schema.annotate(tag(kind)));
/**
 * WIT `u8` (`number`, integer 0..255).
 *
 * @since 1.5.0
 * @category codecs
 */
intPin("u8", 0, 255);
/**
 * WIT `u16` (`number`, integer 0..65535).
 *
 * @since 1.5.0
 * @category codecs
 */
intPin("u16", 0, 65535);
/**
 * WIT `u32` (`number`, integer 0..2^32-1).
 *
 * @since 1.5.0
 * @category codecs
 */
const Uint32 = intPin("u32", 0, 4294967295);
/**
 * WIT `s8` (`number`, integer -128..127).
 *
 * @since 1.5.0
 * @category codecs
 */
intPin("s8", -128, 127);
/**
 * WIT `s16` (`number`, integer -32768..32767).
 *
 * @since 1.5.0
 * @category codecs
 */
intPin("s16", -32768, 32767);
/**
 * WIT `s32` (`number`, integer -2^31..2^31-1).
 *
 * @since 1.5.0
 * @category codecs
 */
intPin("s32", -2147483648, 2147483647);
/**
 * WIT `f32` (`number`, 32-bit float).
 *
 * @since 1.5.0
 * @category codecs
 */
Schema.Number.pipe(Schema.annotate(tag("f32")));
/**
 * WIT `f64` (`number`, 64-bit float). Same default as `Schema.Number`.
 *
 * @since 1.5.0
 * @category codecs
 */
Schema.Number.pipe(Schema.annotate(tag("f64")));
/**
 * WIT `s64` (`bigint`). Same default as `Schema.BigInt`.
 *
 * @since 1.5.0
 * @category codecs
 */
bigPin("s64", -9223372036854775808n, 9223372036854775807n);
/**
 * WIT `u64` (`bigint`, non-negative).
 *
 * @since 1.5.0
 * @category codecs
 */
const Uint64 = bigPin("u64", 0n, 18446744073709551615n);
/**
 * WIT `char` — a single Unicode scalar value, carried as a JS one-character
 * `string`. Built on top of `Schema.Char` so length-1 validation runs as
 * part of the user schema; the codec sees the `effect-golem/witType: "char"`
 * annotation and emits `prim-char-type` / `prim-char` accordingly.
 *
 * @since 1.5.0
 * @category codecs
 */
Schema.Char.pipe(Schema.annotate({ [witTypeAnnotationKey]: "char" }));
/**
 * Annotation key carrying the *name* of a variant case for a
 * `Schema.Union(...)` member compiled to a generic WIT `variant`. When
 * absent, the codec auto-names cases `case0..caseN`.
 *
 * **Example**
 *
 * ```ts
 * Schema.Union(
 *   Schema.String.pipe(withVariantCaseName("text")),
 *   Schema.Number.pipe(withVariantCaseName("count")),
 * )
 * // → variant { text(string), count(f64) }
 * ```
 *
 * @since 1.5.0
 * @category utils
 */
const variantCaseNameAnnotationKey = "effect-golem/variantCaseName";
/**
 * Annotation key carrying a typed-array hint: `"u8" | "i8" | "u16" | "i16"
 * | "u32" | "i32" | "f32" | "f64" | "big-i64" | "big-u64"`.
 *
 * Schemas annotated with this key are emitted by the codec as a WIT
 * `list<primN>` (or `list<sN>`/`list<f32>` …) and reconstructed back to
 * the corresponding TypedArray subclass on decode.
 *
 * @since 1.5.0
 * @category utils
 */
const witTypedArrayAnnotationKey = "effect-golem/witTypedArray";
/**
 * Annotation key marking a schema as the opaque `quota-token` capability
 * node. Schemas carrying this annotation are compiled by the codec to the
 * schema-model `quota-token` type (`t.quotaToken`) and their values bridge the
 * host `QuotaToken` (an owned `own<quota-token>` resource) to/from a
 * `v.quotaToken(handle)` schema value.
 *
 * Unlike the numeric / typed-array hints there is no payload — the presence of
 * the key alone selects the quota-token shape.
 *
 * @since 1.5.0
 * @category utils
 */
const witQuotaTokenAnnotationKey = WIT_QUOTA_TOKEN_ANNOTATION_KEY;
/**
 * Annotation key marking a schema as a `principal` value carried as ordinary
 * structured data (the WIT `golem:agent/common` `principal` variant —
 * `oidc` / `agent` / `golem-user` / `anonymous`). Schemas carrying this
 * annotation are compiled by the codec to that variant type and their values
 * round-trip a host `Principal` to/from the corresponding `SchemaValue`.
 *
 * Unlike the numeric / typed-array hints there is no payload — the presence of
 * the key alone selects the principal shape. See {@link Principal.PrincipalSchema}.
 *
 * @since 1.6.0
 * @category utils
 */
const witPrincipalAnnotationKey = "effect-golem/witPrincipal";
/** Annotation key used by Golem-native rich and capability schemas. @since 1.6.0 @category annotations */
const witSchemaNodeAnnotationKey = "effect-golem/schemaNode";
/** Annotation key for schema-model metadata. @since 1.6.0 @category annotations */
const witMetadataAnnotationKey = "effect-golem/metadata";
const native = (guard, node) => Schema.declare(guard).pipe(Schema.annotate({ [witSchemaNodeAnnotationKey]: node }));
const isDatetime = (u) => typeof u === "object" &&
    u !== null &&
    typeof u.seconds === "bigint" &&
    typeof u.nanoseconds === "number";
/** Canonical datetime schema. @since 1.6.0 @category schemas */
native(isDatetime, { tag: "datetime" });
/** Nanosecond duration schema. @since 1.6.0 @category schemas */
native((u) => typeof u === "bigint", { tag: "duration" });
/**
 * Opaque concrete permission-card capability schema (`permission-card { polymorphic: false }`).
 * It accepts only concrete cards, and decodes a received card as a concrete card.
 *
 * @since 1.6.0
 * @category schemas
 */
native((u) => isGuestPermissionCardHandleOfKind(u, "concrete"), { tag: "permission-card", spec: { polymorphic: false } });
/**
 * Opaque template permission-card capability schema (`permission-card { polymorphic: true }`).
 * It accepts only template cards, and decodes a received card as a template card. The host refuses
 * an agent call input or output whose card does not match the declared kind.
 *
 * @since 1.6.0
 * @category schemas
 */
native((u) => isGuestPermissionCardHandleOfKind(u, "template"), { tag: "permission-card", spec: { polymorphic: true } });
const typedArraySchema = (kind, ctor) => Schema.declare((u) => u instanceof ctor).pipe(Schema.annotate({ [witTypedArrayAnnotationKey]: kind }));
/**
 * WIT `list<u8>` carried as a JS `Uint8Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
typedArraySchema("u8", Uint8Array);
/**
 * WIT `list<s8>` carried as a JS `Int8Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
typedArraySchema("i8", Int8Array);
/**
 * WIT `list<u16>` carried as a JS `Uint16Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
typedArraySchema("u16", Uint16Array);
/**
 * WIT `list<s16>` carried as a JS `Int16Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
typedArraySchema("i16", Int16Array);
/**
 * WIT `list<u32>` carried as a JS `Uint32Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
typedArraySchema("u32", Uint32Array);
/**
 * WIT `list<s32>` carried as a JS `Int32Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
typedArraySchema("i32", Int32Array);
/**
 * WIT `list<f32>` carried as a JS `Float32Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
typedArraySchema("f32", Float32Array);
/**
 * WIT `list<f64>` carried as a JS `Float64Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
typedArraySchema("f64", Float64Array);
/**
 * WIT `list<s64>` carried as a JS `BigInt64Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
typedArraySchema("big-i64", BigInt64Array);
/**
 * WIT `list<u64>` carried as a JS `BigUint64Array`.
 *
 * @since 1.5.0
 * @category codecs
 */
typedArraySchema("big-u64", BigUint64Array);

/** An iterator whose close interrupts and joins an in-flight Effect pull before scope release. */
class AbortableStreamIterable {
    stream;
    dispose;
    scope = Scope.makeUnsafe();
    runPromise;
    pull;
    current;
    pending;
    execution;
    closePromise;
    constructor(stream, context, dispose) {
        this.stream = stream;
        this.dispose = dispose;
        this.runPromise = Effect.runPromiseWith(context);
    }
    [Symbol.asyncIterator]() {
        return this;
    }
    async next() {
        if (this.closePromise)
            return { done: true, value: undefined };
        const buffered = this.current?.next();
        if (buffered && !buffered.done)
            return buffered;
        this.current = undefined;
        const controller = new AbortController();
        this.pending = controller;
        try {
            if (!this.pull) {
                const acquisition = this.runPromise(Stream.toPull(this.stream).pipe(Scope.provide(this.scope)), { signal: controller.signal });
                this.execution = acquisition;
                this.pull = await acquisition;
                if (this.closePromise)
                    return { done: true, value: undefined };
            }
            const pulling = this.runPromise(Effect.exit(Pull.catchDone(this.pull, () => Effect.succeed([]))), { signal: controller.signal });
            this.execution = pulling;
            const exit = await pulling;
            if (Exit.isFailure(exit)) {
                if (this.pending === controller)
                    this.pending = undefined;
                await this.close(exit).catch(() => undefined);
                throw Cause.squash(exit.cause);
            }
            const chunk = exit.value;
            if (this.pending === controller)
                this.pending = undefined;
            if (chunk.length === 0) {
                await this.close(Exit.void);
                return { done: true, value: undefined };
            }
            this.current = chunk[Symbol.iterator]();
            return this.current.next();
        }
        catch (error) {
            if (this.pending === controller)
                this.pending = undefined;
            if (controller.signal.aborted) {
                await this.close();
                return { done: true, value: undefined };
            }
            await this.close(Exit.die(error)).catch(() => undefined);
            throw error;
        }
        finally {
            if (this.pending === controller)
                this.pending = undefined;
        }
    }
    return() {
        return this.close().then(() => ({ done: true, value: undefined }));
    }
    close(exit = Exit.interrupt()) {
        if (!this.closePromise) {
            this.pending?.abort();
            this.closePromise = (async () => {
                await this.execution?.catch(() => undefined);
                try {
                    await this.runPromise(Scope.close(this.scope, exit));
                }
                finally {
                    await this.dispose?.(exit);
                }
            })();
        }
        return this.closePromise;
    }
}

/** Eager ownership must also be released when a producer is never polled. */
const streamDisposals = new WeakMap();
/** Acquired endpoints remain owned across arbitrary lazy Stream composition. */
const HttpStreamOwner = Context.Reference("golem/http/StreamOwner", { defaultValue: () => undefined });

// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
const received = new WeakMap();
const acquiring = new WeakMap();
/** @internal Move a native Effect stream into a recursive schema value. */
function agentStreamToHandle(stream, itemCodec, encodingContext = Context.empty()) {
    return directAgentStreamToHandle(stream, {
        decode: (tree) => decodeFromWire(itemCodec, tree),
        encode: (item) => Schema.encodeEffect(itemCodec)(item).pipe(Effect.flatMap((value) => Effect.tryPromise({
            try: (signal) => restoreEncodedCapabilitiesOnAbort(signal, schemaValueToWitAsync(value, signal)),
            catch: (error) => error,
        }))),
    }, encodingContext);
}
/** @internal Move a stream using a wire item codec. */
function directAgentStreamToHandle(stream, itemCodec, context = Context.empty()) {
    if (!Stream.isStream(stream))
        throw new Error("expected an Effect Stream");
    const state = received.get(stream);
    if (state) {
        assertTransferable(state);
        const endpoint = state.ownership.iterator
            ? { kind: "native", value: iterableFromIterator(state.ownership.iterator) }
            : state.endpoint;
        if (!endpoint)
            throw new Error("schema stream was already transferred");
        return new GuestSchemaValueStreamHandle(STREAM_INTERNAL, endpoint, undefined, undefined, state.ownership);
    }
    const encoded = stream.pipe(Stream.mapEffect((item) => itemCodec.encode(item).pipe(Effect.provide(context))));
    return new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
        kind: "native",
        value: new AbortableStreamIterable(encoded, context, streamDisposals.get(stream)),
    });
}
/** Dispose an unused received endpoint without polling any body item. */
async function disposeAgentStream(stream) {
    const state = received.get(stream);
    if (state?.ownership.available && state.ownership.reservation === undefined) {
        await new ReceivedStreamIterable(state).return();
    }
}
/** @internal Lift a recursive schema-value-stream handle into a native Effect stream. */
function agentStreamFromHandle(handle, itemCodec, decodingContext = Context.empty()) {
    return directAgentStreamFromHandle(handle, {
        decode: (tree) => decodeFromWire(itemCodec, tree),
        encode: (item) => Schema.encodeEffect(itemCodec)(item).pipe(Effect.flatMap((value) => Effect.tryPromise({
            try: (signal) => restoreEncodedCapabilitiesOnAbort(signal, schemaValueToWitAsync(value, signal)),
            catch: (error) => error,
        }))),
    }, decodingContext);
}
/** @internal Lift a wire stream using a wire item codec. */
function directAgentStreamFromHandle(handle, itemCodec, context = Context.empty()) {
    const endpoint = handle.peek();
    if (!endpoint)
        throw new Error("schema value stream was already transferred");
    const state = { endpoint, ownership: handle.ownership(STREAM_INTERNAL) };
    const stream = Stream.unwrap(Effect.acquireRelease(Effect.try({
        try: () => {
            assertTransferable(state);
            state.ownership.reader = {};
            return new ReceivedStreamIterable(state);
        },
        catch: (error) => error,
    }), (iterable) => Effect.promise(() => iterable.return()).pipe(Effect.asVoid)).pipe(Effect.map((iterable) => Stream.unfold(undefined, () => Effect.tryPromise({
        try: (signal) => new Promise((resolve, reject) => {
            signal.addEventListener("abort", () => resolve({ done: true, value: undefined }), {
                once: true,
            });
            iterable.next().then(resolve, reject);
        }),
        catch: (error) => error,
    }).pipe(Effect.flatMap((item) => item.done
        ? Effect.succeed(undefined)
        : itemCodec.decode(item.value).pipe(Effect.provide(context), Effect.map((value) => [value, undefined]))), Effect.catch((error) => error instanceof StreamError && error.failure.tag === "cancelled"
        ? Effect.interrupt
        : Effect.fail(error))))))).pipe(Stream.mapError(StreamError.from));
    received.set(stream, state);
    Context.get(context, HttpStreamOwner)?.add(() => disposeAgentStream(stream));
    return stream;
}
class ReceivedStreamIterable {
    state;
    constructor(state) {
        this.state = state;
    }
    [Symbol.asyncIterator]() {
        return this;
    }
    async next() {
        const state = this.available();
        if (state.ownership.busy)
            throw new Error("a schema stream operation is already in progress");
        state.ownership.busy = true;
        try {
            const item = await (await wireIterator(state)).next();
            if (item.done) {
                state.ownership.available = false;
                state.closed ??= Promise.resolve();
                return { done: true, value: undefined };
            }
            return item;
        }
        finally {
            state.ownership.busy = false;
        }
    }
    return() {
        const state = this.state;
        if (!state.closed) {
            this.available();
            state.ownership.available = false;
            state.closed = Promise.resolve().then(async () => {
                const iterator = await wireIterator(state);
                await iterator.return?.();
            });
        }
        return state.closed.then(() => ({ done: true, value: undefined }));
    }
    available() {
        assertCapabilityReady(this.state.ownership);
        if (!this.state.ownership.available)
            throw new Error("schema stream was already transferred or closed");
        if (this.state.ownership.reservation !== undefined)
            throw new Error("schema stream is reserved for transfer");
        return this.state;
    }
}
function assertTransferable(state) {
    assertCapabilityReady(state.ownership);
    if (state.ownership.reader)
        throw new Error("schema stream already has a reader or is closed");
    if (state.ownership.busy)
        throw new Error("cannot transfer a schema stream while reading");
    if (state.ownership.reservation !== undefined || !state.ownership.available)
        throw new Error("schema stream was already transferred or reserved");
}
async function wireIterator(state) {
    if (state.ownership.iterator)
        return state.ownership.iterator;
    const pending = acquiring.get(state.ownership);
    if (pending)
        return pending;
    const endpoint = state.endpoint;
    if (!endpoint)
        throw new Error("schema stream was already transferred");
    state.endpoint = undefined;
    const acquisition = (async () => {
        const source = endpoint.kind === "native" ? endpoint.value : await unwrapSchemaValueStream(endpoint.value);
        state.ownership.iterator = source[Symbol.asyncIterator]();
        return state.ownership.iterator;
    })();
    acquiring.set(state.ownership, acquisition);
    return acquisition;
}
function iterableFromIterator(iterator) {
    return { [Symbol.asyncIterator]: () => iterator };
}

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
class SchemaRenderError extends TypeError {
    path;
    _tag = "SchemaRenderError";
    constructor(path, message) {
        super(message);
        this.path = path;
    }
}
function datetimeFromISOString(value) {
    const match = value.match(/^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?Z$/);
    if (!match)
        throw new Error(`Invalid UTC datetime '${value}'`);
    const wholeSeconds = `${match[1]}-${match[2]}-${match[3]}T${match[4]}:${match[5]}:${match[6]}`;
    const milliseconds = Date.parse(`${wholeSeconds}Z`);
    if (!Number.isFinite(milliseconds) ||
        new Date(milliseconds).toISOString().slice(0, 19) !== wholeSeconds)
        throw new Error(`Invalid datetime '${value}'`);
    return {
        seconds: BigInt(milliseconds / 1000),
        nanoseconds: Number((match[7] ?? "").padEnd(9, "0") || 0),
    };
}
function datetimeToISOString(value) {
    if (!Number.isInteger(value.nanoseconds) ||
        value.nanoseconds < 0 ||
        value.nanoseconds >= 1_000_000_000)
        throw new Error(`Datetime nanoseconds out of range: ${value.nanoseconds}`);
    if (value.seconds < -62167219200n || value.seconds > 253402300799n)
        throw new Error(`Datetime seconds outside canonical year range 0000..9999: ${value.seconds}`);
    const base = new Date(Number(value.seconds * 1000n)).toISOString().slice(0, 19);
    const fraction = value.nanoseconds === 0
        ? ""
        : `.${value.nanoseconds.toString().padStart(9, "0").replace(/0+$/, "")}`;
    return `${base}${fraction}Z`;
}
function fail$1(path, message) {
    throw new SchemaRenderError(path, message);
}
function resolve(graph, type, seen = new Set()) {
    if (type.body.tag !== "ref")
        return type;
    if (seen.has(type.body.id))
        throw new SchemaRenderError([], `reference cycle through '${type.body.id}'`);
    const definition = graph.defs.get(type.body.id);
    if (!definition)
        throw new SchemaRenderError([], `dangling reference '${type.body.id}'`);
    seen.add(type.body.id);
    return resolve(graph, definition.body, seen);
}
function fromCanonicalJson(graph, type, json, path = [], fieldDefaults) {
    const body = resolve(graph, type).body;
    switch (body.tag) {
        case "bool":
            return { tag: "bool", value: expectBoolean(json, path) };
        case "s8":
            return { tag: "s8", value: expectInteger(json, path, -128, 127) };
        case "s16":
            return { tag: "s16", value: expectInteger(json, path, -32768, 32767) };
        case "s32":
            return { tag: "s32", value: expectInteger(json, path, -2147483648, 2 ** 31 - 1) };
        case "u8":
            return { tag: "u8", value: expectInteger(json, path, 0, 255) };
        case "u16":
            return { tag: "u16", value: expectInteger(json, path, 0, 65535) };
        case "u32":
            return { tag: "u32", value: expectInteger(json, path, 0, 2 ** 32 - 1) };
        case "s64":
            return { tag: "s64", value: expectCanonicalInteger(json, path, I64_MIN, I64_MAX, "s64") };
        case "u64":
            return { tag: "u64", value: expectCanonicalInteger(json, path, 0n, U64_MAX$1, "u64") };
        case "f32":
            return { tag: "f32", value: expectNumber(Math.fround(expectNumber(json, path)), path) };
        case "f64":
            return { tag: "f64", value: expectNumber(json, path) };
        case "char": {
            const value = expectString(json, path);
            if ([...value].length !== 1)
                fail$1(path, "expected one Unicode scalar");
            return { tag: "char", value };
        }
        case "string":
            return { tag: "string", value: expectString(json, path) };
        case "text":
            return decodeText(json, path);
        case "binary":
            return decodeBinary(json, path);
        case "path":
            return { tag: "path", value: expectNonEmptyString(json, path) };
        case "url":
            return { tag: "url", value: expectNonEmptyString(json, path) };
        case "uuid":
            try {
                return { tag: "uuid", value: parseUuid(expectString(json, path)) };
            }
            catch {
                return fail$1(path, "invalid UUID");
            }
        case "datetime":
            return { tag: "datetime", value: datetimeFromISOString(expectString(json, path)) };
        case "duration":
            return { tag: "duration", nanoseconds: decodeDuration(json, path) };
        case "quantity":
            return decodeQuantity(json, path);
        case "record": {
            const object = expectObject(json, path);
            const expected = new Set(body.fields.map((field) => field.name));
            for (const key of Object.keys(object))
                if (!expected.has(key))
                    fail$1([...path, key], "unknown field");
            return {
                tag: "record",
                fields: body.fields.map((field) => {
                    if (!Object.prototype.hasOwnProperty.call(object, field.name)) {
                        const fallback = fieldDefaults?.get(field.name);
                        if (fallback !== undefined)
                            return cloneSchemaValue(fallback);
                        if (resolve(graph, field.body).body.tag === "option")
                            return { tag: "option" };
                        fail$1([...path, field.name], `missing field '${field.name}'`);
                    }
                    return fromCanonicalJson(graph, field.body, object[field.name], [...path, field.name]);
                }),
            };
        }
        case "variant": {
            if (typeof json === "string") {
                const caseIndex = body.cases.findIndex((entry) => entry.name === json && !entry.payload);
                if (caseIndex < 0)
                    fail$1(path, `unknown payload-free variant case '${json}'`);
                return { tag: "variant", caseIndex };
            }
            const object = expectObject(json, path);
            const entries = Object.entries(object);
            if (entries.length !== 1)
                fail$1(path, "expected a single-key variant object");
            const [name, payload] = entries[0];
            const caseIndex = body.cases.findIndex((entry) => entry.name === name);
            const variantCase = body.cases[caseIndex];
            if (!variantCase?.payload)
                fail$1(path, `unknown payload variant case '${name}'`);
            return {
                tag: "variant",
                caseIndex,
                payload: fromCanonicalJson(graph, variantCase.payload, payload, [...path, name]),
            };
        }
        case "enum": {
            const name = expectString(json, path);
            const caseIndex = body.cases.indexOf(name);
            if (caseIndex < 0)
                fail$1(path, `unknown enum case '${name}'`);
            return { tag: "enum", caseIndex };
        }
        case "flags": {
            const selected = expectArray(json, path).map((entry, index) => expectString(entry, [...path, index]));
            const seen = new Set();
            selected.forEach((name, index) => {
                if (!body.names.includes(name))
                    fail$1([...path, index], `unknown flag '${name}'`);
                if (seen.has(name))
                    fail$1([...path, index], `duplicate flag '${name}'`);
                seen.add(name);
            });
            return { tag: "flags", flags: body.names.map((name) => selected.includes(name)) };
        }
        case "tuple": {
            const values = expectArray(json, path);
            if (values.length !== body.elements.length)
                fail$1(path, `expected ${body.elements.length} tuple elements`);
            return {
                tag: "tuple",
                elements: body.elements.map((entry, index) => fromCanonicalJson(graph, entry, values[index], [...path, index])),
            };
        }
        case "list":
        case "fixed-list": {
            const values = expectArray(json, path);
            if (body.tag === "fixed-list" && values.length !== body.length)
                fail$1(path, `expected ${body.length} elements`);
            const elements = values.map((entry, index) => fromCanonicalJson(graph, body.element, entry, [...path, index]));
            return body.tag === "list" ? { tag: "list", elements } : { tag: "fixed-list", elements };
        }
        case "map": {
            const entries = expectArray(json, path).map((entry, index) => {
                const pair = expectArray(entry, [...path, index]);
                if (pair.length !== 2)
                    fail$1([...path, index], "expected a two-element map entry");
                return {
                    key: fromCanonicalJson(graph, body.key, pair[0], [...path, index, 0]),
                    value: fromCanonicalJson(graph, body.value, pair[1], [...path, index, 1]),
                };
            });
            return { tag: "map", entries };
        }
        case "option":
            return json === null
                ? { tag: "option" }
                : { tag: "option", value: fromCanonicalJson(graph, body.element, json, path) };
        case "result": {
            const object = expectObject(json, path);
            const keys = Object.keys(object);
            if (keys.length !== 1 || (keys[0] !== "ok" && keys[0] !== "err"))
                fail$1(path, "expected {'ok': ...} or {'err': ...}");
            const tag = keys[0];
            const expected = tag === "ok" ? body.ok : body.err;
            const payload = object[tag];
            if (!expected) {
                if (payload !== null)
                    fail$1([...path, tag], "expected null unit payload");
                return { tag: "result", result: { tag } };
            }
            return {
                tag: "result",
                result: { tag, value: fromCanonicalJson(graph, expected, payload, [...path, tag]) },
            };
        }
        case "union": {
            const matches = body.branches.filter((branch) => discriminatorMatches$2(branch.discriminator, json));
            if (matches.length !== 1)
                fail$1(path, `expected exactly one matching union branch, found ${matches.length}`);
            return {
                tag: "union",
                unionTag: matches[0].tag,
                body: fromCanonicalJson(graph, matches[0].body, json, path),
            };
        }
        case "secret":
        case "quota-token":
        case "permission-card":
            return fail$1(path, `${body.tag} values cannot be constructed from JSON`);
        case "future":
        case "stream":
            return fail$1(path, `${body.tag} values have no JSON representation`);
        case "ref":
            throw new Error("unreachable");
    }
}
function toCanonicalJson(graph, type, value, path = []) {
    const body = resolve(graph, type).body;
    if (body.tag === "secret" || body.tag === "quota-token" || body.tag === "permission-card")
        fail$1(path, `${body.tag} values cannot be exposed as JSON`);
    if (body.tag === "future" || body.tag === "stream")
        fail$1(path, `${body.tag} values have no JSON representation`);
    if (body.tag !== value.tag && !(body.tag === "ref"))
        fail$1(path, `expected ${body.tag} schema value, found ${value.tag}`);
    switch (value.tag) {
        case "bool":
        case "s8":
        case "s16":
        case "s32":
        case "u8":
        case "u16":
        case "u32":
            return value.value;
        case "f32":
        case "f64":
            return expectNumber(value.value, path);
        case "s64":
            return checkedIntegerString(value.value, path, I64_MIN, I64_MAX, "s64");
        case "u64":
            return checkedIntegerString(value.value, path, 0n, U64_MAX$1, "u64");
        case "char":
        case "string":
        case "path":
        case "url":
            return value.value;
        case "uuid":
            return uuidToString(value.value);
        case "text":
            return {
                text: value.text,
                ...(value.language === undefined ? {} : { language: value.language }),
            };
        case "binary": {
            if (value.mimeType !== undefined && !MIME_TYPE_PATTERN.test(value.mimeType))
                fail$1([...path, "mimeType"], "invalid MIME type");
            return {
                bytes: bytesToBase64(value.bytes),
                ...(value.mimeType === undefined ? {} : { mimeType: value.mimeType }),
            };
        }
        case "datetime":
            return datetimeToISOString(value.value);
        case "duration":
            return {
                nanoseconds: checkedIntegerString(value.nanoseconds, [...path, "nanoseconds"], I64_MIN, I64_MAX, "duration nanoseconds"),
            };
        case "quantity":
            return {
                mantissa: checkedIntegerString(value.value.mantissa, [...path, "mantissa"], I64_MIN, I64_MAX, "quantity mantissa"),
                scale: value.value.scale,
                unit: value.value.unit,
            };
        case "record": {
            const fields = body.fields;
            if (value.fields.length !== fields.length)
                fail$1(path, `expected ${fields.length} record fields, found ${value.fields.length}`);
            return Object.fromEntries(fields.map((field, index) => [
                field.name,
                toCanonicalJson(graph, field.body, value.fields[index], [...path, field.name]),
            ]));
        }
        case "variant": {
            const entry = body.cases[value.caseIndex];
            if (!entry)
                fail$1(path, `variant case index ${value.caseIndex} is out of range`);
            return value.payload === undefined
                ? entry.name
                : {
                    [entry.name]: toCanonicalJson(graph, entry.payload, value.payload, [
                        ...path,
                        entry.name,
                    ]),
                };
        }
        case "enum":
            return (body.cases[value.caseIndex] ??
                fail$1(path, "enum case index is out of range"));
        case "flags": {
            const names = body.names;
            if (value.flags.length !== names.length)
                fail$1(path, `expected ${names.length} flag values, found ${value.flags.length}`);
            return names.filter((_, index) => value.flags[index]);
        }
        case "tuple": {
            const elements = body.elements;
            if (value.elements.length !== elements.length)
                fail$1(path, `expected ${elements.length} tuple elements, found ${value.elements.length}`);
            return value.elements.map((entry, index) => toCanonicalJson(graph, elements[index], entry, [...path, index]));
        }
        case "list":
        case "fixed-list":
            if (body.tag === "fixed-list" && value.elements.length !== body.length)
                fail$1(path, `expected ${body.length} elements, found ${value.elements.length}`);
            return value.elements.map((entry, index) => toCanonicalJson(graph, body.element, entry, [...path, index]));
        case "map":
            return value.entries.map((entry, index) => [
                toCanonicalJson(graph, body.key, entry.key, [
                    ...path,
                    index,
                    0,
                ]),
                toCanonicalJson(graph, body.value, entry.value, [...path, index, 1]),
            ]);
        case "option":
            return value.value === undefined
                ? null
                : toCanonicalJson(graph, body.element, value.value, path);
        case "result": {
            const expected = value.result.tag === "ok"
                ? body.ok
                : body.err;
            return {
                [value.result.tag]: value.result.value === undefined
                    ? null
                    : toCanonicalJson(graph, expected, value.result.value, [...path, value.result.tag]),
            };
        }
        case "union":
            return toCanonicalJson(graph, body.branches.find((entry) => entry.tag === value.unionTag)?.body ?? fail$1(path, `unknown union tag '${value.unionTag}'`), value.body, path);
        case "secret":
        case "quota-token":
        case "permission-card":
            return fail$1(path, `${value.tag} values cannot be exposed as JSON`);
        case "stream":
            fail$1(path, "stream values have no JSON representation");
    }
}
function toCanonicalJsonSchema(graph, type, includeDraftMarker, fieldDefaults) {
    const referencedDefinitions = assertCanonicalJsonEligible(graph, type, true);
    const rendered = renderSchema(graph, type);
    const root = fieldDefaults?.size
        ? withFieldDefaults(graph, type, rendered, fieldDefaults)
        : rendered;
    const defs = Object.fromEntries([...referencedDefinitions].map((id) => {
        const definition = graph.defs.get(id);
        const rendered = renderSchema(graph, definition.body);
        return [
            id,
            definition.name === undefined || rendered.title !== undefined
                ? rendered
                : { ...rendered, title: definition.name },
        ];
    }));
    return {
        ...({}),
        ...root,
        ...(Object.keys(defs).length ? { $defs: defs } : {}),
    };
}
function withFieldDefaults(graph, type, rendered, fieldDefaults) {
    const body = type.body;
    if (body.tag !== "record")
        return rendered;
    const properties = { ...rendered.properties };
    for (const field of body.fields) {
        const value = fieldDefaults.get(field.name);
        if (value === undefined)
            continue;
        let json;
        try {
            json = toCanonicalJson(graph, field.body, value);
        }
        catch {
            continue;
        }
        properties[field.name] = {
            ...properties[field.name],
            default: json,
        };
    }
    return {
        ...rendered,
        properties,
        required: rendered.required.filter((name) => !fieldDefaults.has(name)),
    };
}
function assertCanonicalJsonEligible(graph, root, allowUnsupportedLeaves = false) {
    const referencedDefinitions = new Set();
    const visit = (type, path, aliasChain = new Set()) => {
        const body = type.body;
        switch (body.tag) {
            case "ref": {
                const definition = graph.defs.get(body.id);
                if (!definition)
                    fail$1(path, `dangling reference '${body.id}'`);
                if (aliasChain.has(body.id))
                    fail$1(path, `reference cycle through '${body.id}' has no structural JSON value`);
                if (referencedDefinitions.has(body.id))
                    return;
                referencedDefinitions.add(body.id);
                visit(definition.body, [...path, `$ref:${body.id}`], new Set([...aliasChain, body.id]));
                return;
            }
            case "record":
                body.fields.forEach((field) => visit(field.body, [...path, field.name]));
                return;
            case "variant":
                body.cases.forEach((entry) => {
                    if (entry.payload)
                        visit(entry.payload, [...path, entry.name]);
                });
                return;
            case "tuple":
                body.elements.forEach((element, index) => visit(element, [...path, index]));
                return;
            case "list":
            case "fixed-list":
                visit(body.element, [...path, "items"]);
                return;
            case "map":
                visit(body.key, [...path, "key"]);
                visit(body.value, [...path, "value"]);
                return;
            case "option":
                if (canRenderNull(graph, body.element))
                    fail$1(path, "nested options cannot preserve None versus Some(None) in canonical JSON");
                visit(body.element, path);
                return;
            case "result":
                if (body.ok)
                    visit(body.ok, [...path, "ok"]);
                if (body.err)
                    visit(body.err, [...path, "err"]);
                return;
            case "union":
                body.branches.forEach((branch) => visit(branch.body, [...path, branch.tag]));
                return;
            case "secret":
            case "quota-token":
            case "permission-card":
                if (allowUnsupportedLeaves)
                    return;
                return fail$1(path, `${body.tag} values cannot cross a canonical JSON boundary`);
            case "future":
            case "stream":
                if (allowUnsupportedLeaves)
                    return;
                return fail$1(path, `${body.tag} values have no canonical JSON representation`);
            default:
                return;
        }
    };
    visit(root, []);
    return referencedDefinitions;
}
function canRenderNull(graph, type, seen = new Set()) {
    switch (type.body.tag) {
        case "option":
            return true;
        case "union":
            return type.body.branches.some((branch) => canRenderNull(graph, branch.body, seen));
        case "ref": {
            if (seen.has(type.body.id))
                return false;
            const definition = graph.defs.get(type.body.id);
            return definition
                ? canRenderNull(graph, definition.body, new Set([...seen, type.body.id]))
                : false;
        }
        default:
            return false;
    }
}
function renderSchema(graph, type) {
    const body = type.body;
    if (body.tag === "ref")
        return { $ref: `#/$defs/${body.id.replaceAll("~", "~0").replaceAll("/", "~1")}` };
    let rendered;
    switch (body.tag) {
        case "bool":
            rendered = { type: "boolean" };
            break;
        case "s8":
            rendered = integerSchema(-128, 127);
            break;
        case "s16":
            rendered = integerSchema(-32768, 32767);
            break;
        case "s32":
            rendered = integerSchema(-2147483648, 2 ** 31 - 1);
            break;
        case "s64":
            rendered = integerStringSchema(restrictedIntegerBound(body.restrictions?.min, I64_MIN, "min"), restrictedIntegerBound(body.restrictions?.max, I64_MAX, "max"), true, "int64");
            break;
        case "u8":
            rendered = integerSchema(0, 255);
            break;
        case "u16":
            rendered = integerSchema(0, 65535);
            break;
        case "u32":
            rendered = integerSchema(0, 2 ** 32 - 1);
            break;
        case "u64":
            rendered = integerStringSchema(restrictedIntegerBound(body.restrictions?.min, 0n, "min"), restrictedIntegerBound(body.restrictions?.max, U64_MAX$1, "max"), false, "uint64");
            break;
        case "f32":
        case "f64":
            rendered = { type: "number" };
            break;
        case "char":
            rendered = { type: "string", minLength: 1, maxLength: 1 };
            break;
        case "string":
            rendered = { type: "string" };
            break;
        case "text": {
            const text = { type: "string" };
            if (body.restrictions.minLength !== undefined)
                text.minLength = body.restrictions.minLength;
            if (body.restrictions.maxLength !== undefined)
                text.maxLength = body.restrictions.maxLength;
            if (body.restrictions.regex !== undefined)
                text.pattern = body.restrictions.regex;
            rendered = {
                type: "object",
                properties: {
                    text,
                    language: {
                        type: "string",
                        ...(body.restrictions.languages === undefined
                            ? {}
                            : { enum: body.restrictions.languages }),
                    },
                },
                required: ["text"],
                additionalProperties: false,
            };
            break;
        }
        case "binary":
            rendered = {
                type: "object",
                required: ["bytes"],
                properties: {
                    bytes: {
                        type: "string",
                        contentEncoding: "base64url",
                        pattern: BASE64URL_PATTERN,
                        ...(body.restrictions.minBytes === undefined
                            ? {}
                            : { minLength: base64UrlLength(body.restrictions.minBytes) }),
                        ...(body.restrictions.maxBytes === undefined
                            ? {}
                            : { maxLength: base64UrlLength(body.restrictions.maxBytes) }),
                    },
                    mimeType: {
                        type: "string",
                        pattern: MIME_TYPE_PATTERN.source,
                        ...(body.restrictions.mimeTypes === undefined
                            ? {}
                            : { enum: body.restrictions.mimeTypes }),
                    },
                },
                additionalProperties: false,
            };
            break;
        case "path": {
            const direction = body.spec.direction === "in-out" ? "inout" : body.spec.direction;
            const descriptions = [
                body.spec.allowedExtensions === undefined
                    ? undefined
                    : `Allowed extensions: ${body.spec.allowedExtensions.join(", ")}`,
                body.spec.allowedMimeTypes === undefined
                    ? undefined
                    : `Allowed MIME types: ${body.spec.allowedMimeTypes.join(", ")}`,
            ].filter((entry) => entry !== undefined);
            rendered = {
                type: "string",
                format: "file-path",
                title: `${direction} ${body.spec.kind} path`,
                ...(descriptions.length ? { description: descriptions.join("; ") } : {}),
            };
            break;
        }
        case "url": {
            const descriptions = [
                body.restrictions.allowedSchemes === undefined
                    ? undefined
                    : `Allowed schemes: ${body.restrictions.allowedSchemes.join(", ")}`,
                body.restrictions.allowedHosts === undefined
                    ? undefined
                    : `Allowed hosts: ${body.restrictions.allowedHosts.join(", ")}`,
            ].filter((entry) => entry !== undefined);
            rendered = {
                type: "string",
                format: "uri",
                title: "URL",
                ...(descriptions.length ? { description: descriptions.join("; ") } : {}),
            };
            break;
        }
        case "uuid":
            rendered = { type: "string", format: "uuid", title: "UUID" };
            break;
        case "datetime":
            rendered = { type: "string", format: "date-time" };
            break;
        case "duration":
            rendered = {
                type: "object",
                required: ["nanoseconds"],
                properties: {
                    nanoseconds: integerStringSchema(I64_MIN, I64_MAX, true, "int64"),
                },
                additionalProperties: false,
                title: "Duration in nanoseconds",
            };
            break;
        case "quantity":
            rendered = {
                type: "object",
                required: ["mantissa", "scale", "unit"],
                properties: {
                    mantissa: integerStringSchema(I64_MIN, I64_MAX, true, "int64"),
                    scale: { type: "integer" },
                    unit: { type: "string" },
                },
                additionalProperties: false,
                title: `Quantity (${body.spec.baseUnit})`,
            };
            break;
        case "record":
            rendered = {
                type: "object",
                properties: Object.fromEntries(body.fields.map((field) => [
                    field.name,
                    attachMetadata(renderSchema(graph, field.body), field.metadata),
                ])),
                required: body.fields
                    .filter((field) => resolve(graph, field.body).body.tag !== "option")
                    .map((field) => field.name),
                additionalProperties: false,
            };
            break;
        case "variant":
            rendered = {
                oneOf: body.cases.map((entry) => {
                    if (!entry.payload)
                        return { const: entry.name };
                    return {
                        type: "object",
                        required: [entry.name],
                        properties: { [entry.name]: renderSchema(graph, entry.payload) },
                        additionalProperties: false,
                    };
                }),
            };
            break;
        case "enum":
            rendered = { type: "string", enum: body.cases };
            break;
        case "flags":
            rendered = { type: "array", items: { type: "string", enum: body.names }, uniqueItems: true };
            break;
        case "tuple":
            rendered =
                body.elements.length === 0
                    ? { type: "array", minItems: 0, maxItems: 0 }
                    : {
                        type: "array",
                        prefixItems: body.elements.map((entry) => renderSchema(graph, entry)),
                        items: false,
                        minItems: body.elements.length,
                    };
            break;
        case "list":
            rendered = { type: "array", items: renderSchema(graph, body.element) };
            break;
        case "fixed-list":
            rendered = {
                type: "array",
                items: renderSchema(graph, body.element),
                minItems: body.length,
                maxItems: body.length,
            };
            break;
        case "map":
            rendered = {
                type: "array",
                items: {
                    type: "array",
                    prefixItems: [renderSchema(graph, body.key), renderSchema(graph, body.value)],
                    items: false,
                    minItems: 2,
                    maxItems: 2,
                },
            };
            break;
        case "option":
            rendered = { oneOf: [{ type: "null" }, renderSchema(graph, body.element)] };
            break;
        case "result":
            rendered = {
                oneOf: [
                    {
                        type: "object",
                        required: ["ok"],
                        properties: { ok: body.ok ? renderSchema(graph, body.ok) : { type: "null" } },
                        additionalProperties: false,
                    },
                    {
                        type: "object",
                        required: ["err"],
                        properties: { err: body.err ? renderSchema(graph, body.err) : { type: "null" } },
                        additionalProperties: false,
                    },
                ],
            };
            break;
        case "union":
            rendered = {
                oneOf: body.branches.map((entry) => applyDiscriminator(renderSchema(graph, entry.body), entry.discriminator)),
            };
            break;
        case "secret":
        case "quota-token":
        case "permission-card":
        case "future":
        case "stream":
            rendered = { not: {} };
            break;
    }
    if ((rendered.type === "integer" || rendered.type === "number") && "restrictions" in body) {
        const bounds = body.restrictions;
        if (bounds?.min !== undefined)
            rendered.minimum = Math.max(bounds.min.tag === "float-bits" ? floatFromBits(bounds.min.val) : Number(bounds.min.val), rendered.minimum ?? -Infinity);
        if (bounds?.max !== undefined)
            rendered.maximum = Math.min(bounds.max.tag === "float-bits" ? floatFromBits(bounds.max.val) : Number(bounds.max.val), rendered.maximum ?? Infinity);
    }
    return attachMetadata(rendered, type.metadata);
}
function integerSchema(minimum, maximum) {
    return { type: "integer", minimum, maximum };
}
function base64UrlLength(bytes) {
    return 4 * Math.floor(bytes / 3) + (bytes % 3 === 0 ? 0 : bytes % 3 === 1 ? 2 : 3);
}
function attachMetadata(schema, metadata) {
    return {
        ...schema,
        ...(metadata.doc === undefined || schema.description !== undefined
            ? {}
            : { description: metadata.doc }),
        ...(metadata.examples.length === 0 || schema.examples !== undefined
            ? {}
            : { examples: metadata.examples }),
        ...(metadata.deprecated === undefined
            ? {}
            : {
                deprecated: true,
                "x-golem-deprecation-note": metadata.deprecated,
            }),
    };
}
function applyDiscriminator(schema, rule) {
    let condition;
    switch (rule.tag) {
        case "prefix":
            condition = { type: "string", pattern: `^${escapeRegex(rule.val)}` };
            break;
        case "suffix":
            condition = { type: "string", pattern: `${escapeRegex(rule.val)}$` };
            break;
        case "contains":
            condition = { type: "string", pattern: escapeRegex(rule.val) };
            break;
        case "regex":
            condition = { type: "string", pattern: rule.val };
            break;
        case "field-equals": {
            const field = rule.val;
            condition = {
                type: "object",
                required: [field.fieldName],
                ...(field.literal === undefined
                    ? {}
                    : { properties: { [field.fieldName]: { const: field.literal } } }),
            };
            break;
        }
        case "field-absent":
            condition = { type: "object", not: { required: [rule.val] } };
            break;
        default:
            return schema;
    }
    return { allOf: [schema, condition] };
}
function escapeRegex(value) {
    return value.replace(/[\\^$.*+?()[\]{}|]/gu, "\\$&");
}
function expectBoolean(value, path) {
    if (typeof value !== "boolean")
        fail$1(path, "expected a JSON boolean");
    return value;
}
function expectNumber(value, path) {
    if (typeof value !== "number" || !Number.isFinite(value))
        fail$1(path, "expected a finite JSON number");
    return value;
}
function expectSafeInteger(value, path) {
    const n = expectNumber(value, path);
    if (!Number.isSafeInteger(n))
        fail$1(path, "expected a safe JSON integer");
    return n;
}
function expectInteger(value, path, min, max) {
    const n = expectSafeInteger(value, path);
    if (n < min || n > max)
        fail$1(path, `integer outside ${min}..${max}`);
    return n;
}
function expectString(value, path) {
    if (typeof value !== "string")
        fail$1(path, "expected a JSON string");
    return value;
}
function expectNonEmptyString(value, path) {
    const string = expectString(value, path);
    if (string.length === 0)
        fail$1(path, "expected a non-empty JSON string");
    return string;
}
function expectArray(value, path) {
    if (!Array.isArray(value))
        fail$1(path, "expected a JSON array");
    return value;
}
function expectObject(value, path) {
    if (value === null || Array.isArray(value) || typeof value !== "object")
        fail$1(path, "expected a JSON object");
    return value;
}
function decodeText(value, path) {
    const object = expectObject(value, path);
    rejectUnknownFields(object, ["text", "language"], path);
    return {
        tag: "text",
        text: expectString(object.text, [...path, "text"]),
        ...(object.language === undefined
            ? {}
            : { language: expectString(object.language, [...path, "language"]) }),
    };
}
function decodeBinary(value, path) {
    const object = expectObject(value, path);
    rejectUnknownFields(object, ["bytes", "mimeType"], path);
    const mimeType = object.mimeType === undefined ? undefined : expectString(object.mimeType, [...path, "mimeType"]);
    if (mimeType !== undefined && !MIME_TYPE_PATTERN.test(mimeType)) {
        fail$1([...path, "mimeType"], "invalid MIME type");
    }
    return {
        tag: "binary",
        bytes: base64UrlToBytes(expectString(object.bytes, [...path, "bytes"]), [...path, "bytes"]),
        ...(mimeType === undefined ? {} : { mimeType }),
    };
}
function decodeQuantity(value, path) {
    const object = expectObject(value, path);
    rejectUnknownFields(object, ["mantissa", "scale", "unit"], path);
    return {
        tag: "quantity",
        value: {
            mantissa: expectCanonicalInteger(object.mantissa, [...path, "mantissa"], I64_MIN, I64_MAX, "quantity mantissa"),
            scale: expectInteger(object.scale, [...path, "scale"], -2147483648, 2147483647),
            unit: expectString(object.unit, [...path, "unit"]),
        },
    };
}
const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;
const U64_MAX$1 = 2n ** 64n - 1n;
const CANONICAL_SIGNED_PATTERN = "^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$";
const CANONICAL_UNSIGNED_PATTERN = "^(?:0|[1-9][0-9]*)$";
function decodeDuration(value, path) {
    const object = expectObject(value, path);
    rejectUnknownFields(object, ["nanoseconds"], path);
    if (!("nanoseconds" in object))
        fail$1([...path, "nanoseconds"], "missing field 'nanoseconds'");
    return expectCanonicalInteger(object.nanoseconds, [...path, "nanoseconds"], I64_MIN, I64_MAX, "duration nanoseconds");
}
function discriminatorMatches$2(rule, value) {
    if (rule.tag === "prefix")
        return typeof value === "string" && value.startsWith(rule.val);
    if (rule.tag === "suffix")
        return typeof value === "string" && value.endsWith(rule.val);
    if (rule.tag === "contains")
        return typeof value === "string" && value.includes(rule.val);
    if (rule.tag === "regex")
        return typeof value === "string" && new RegExp(rule.val, "u").test(value);
    if (rule.tag === "field-equals") {
        const field = rule.val;
        return (value !== null &&
            !Array.isArray(value) &&
            typeof value === "object" &&
            field.fieldName in value &&
            (field.literal === undefined ||
                value[field.fieldName] === field.literal));
    }
    if (rule.tag === "field-absent") {
        return (value !== null &&
            !Array.isArray(value) &&
            typeof value === "object" &&
            !(rule.val in value));
    }
    return false;
}
const MIME_TYPE_PATTERN = new RegExp("^[A-Za-z0-9!#$&^_.+\\-]+\\/[A-Za-z0-9!#$&^_.+\\-]+$", "u");
const BASE64URL_PATTERN = "^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-][AQgw]|[A-Za-z0-9_-]{2}[AEIMQUYcgkosw048])?$";
function bytesToBase64(bytes) {
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let result = "";
    for (let index = 0; index < bytes.length; index += 3) {
        const first = bytes[index];
        const second = bytes[index + 1];
        const third = bytes[index + 2];
        result += alphabet[first >> 2];
        result += alphabet[((first & 3) << 4) | ((second ?? 0) >> 4)];
        if (second !== undefined)
            result += alphabet[((second & 15) << 2) | ((third ?? 0) >> 6)];
        if (third !== undefined)
            result += alphabet[third & 63];
    }
    return result;
}
function base64UrlToBytes(value, path) {
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    if (!/^[A-Za-z0-9_-]*$/u.test(value) || value.length % 4 === 1) {
        fail$1(path, "invalid base64url without padding");
    }
    const bytes = [];
    for (let index = 0; index < value.length; index += 4) {
        const a = alphabet.indexOf(value[index]);
        const b = alphabet.indexOf(value[index + 1]);
        const c = value[index + 2] === undefined ? 0 : alphabet.indexOf(value[index + 2]);
        const d = value[index + 3] === undefined ? 0 : alphabet.indexOf(value[index + 3]);
        bytes.push((a << 2) | (b >> 4));
        if (value[index + 2] !== undefined)
            bytes.push(((b & 15) << 4) | (c >> 2));
        if (value[index + 3] !== undefined)
            bytes.push(((c & 3) << 6) | d);
    }
    const result = Uint8Array.from(bytes);
    if (bytesToBase64(result) !== value)
        fail$1(path, "invalid base64url without padding");
    return result;
}
function rejectUnknownFields(object, allowed, path) {
    Object.keys(object).forEach((key) => {
        if (!allowed.includes(key))
            fail$1([...path, key], "unknown field");
    });
}
function expectCanonicalInteger(value, path, min, max, label) {
    const text = expectString(value, path);
    const pattern = min < 0n ? CANONICAL_SIGNED_PATTERN : CANONICAL_UNSIGNED_PATTERN;
    if (!new RegExp(pattern, "u").test(text))
        fail$1(path, `${label} must be a canonical decimal string`);
    const parsed = BigInt(text);
    if (parsed < min || parsed > max)
        fail$1(path, `${label} is out of range`);
    return parsed;
}
function checkedIntegerString(value, path, min, max, label) {
    if (value < min || value > max)
        fail$1(path, `${label} is out of range`);
    return value.toString();
}
function integerStringSchema(min, max, signed, format) {
    return {
        type: "string",
        format,
        pattern: signed ? CANONICAL_SIGNED_PATTERN : CANONICAL_UNSIGNED_PATTERN,
        "x-golem-minimum": min.toString(),
        "x-golem-maximum": max.toString(),
    };
}
function restrictedIntegerBound(bound, fallback, side) {
    if (bound === undefined)
        return fallback;
    if (bound.tag === "float-bits")
        return fallback;
    const value = bound.val;
    return side === "min"
        ? value > fallback
            ? value
            : fallback
        : value < fallback
            ? value
            : fallback;
}

const overlays = new WeakMap();
const jsonFieldDefaults = (schema) => overlays.get(schema);

/** Validate a WIT value tree without lifting or transferring any affine resources. */
function schemaValueTreeConforms(graph, type, tree) {
    const onPath = new Set();
    const reached = new Set();
    const capabilities = new Set();
    const decode = (index) => {
        if (!Number.isInteger(index) ||
            index < 0 ||
            index >= tree.valueNodes.length ||
            onPath.has(index))
            throw new TypeError("invalid schema value node reference");
        onPath.add(index);
        reached.add(index);
        const node = tree.valueNodes[index];
        const child = (i) => decode(i);
        let value;
        switch (node.tag) {
            case "bool-value":
                value = { tag: "bool", value: node.val };
                break;
            case "s8-value":
                value = { tag: "s8", value: node.val };
                break;
            case "s16-value":
                value = { tag: "s16", value: node.val };
                break;
            case "s32-value":
                value = { tag: "s32", value: node.val };
                break;
            case "s64-value":
                value = { tag: "s64", value: node.val };
                break;
            case "u8-value":
                value = { tag: "u8", value: node.val };
                break;
            case "u16-value":
                value = { tag: "u16", value: node.val };
                break;
            case "u32-value":
                value = { tag: "u32", value: node.val };
                break;
            case "u64-value":
                value = { tag: "u64", value: node.val };
                break;
            case "f32-value":
                value = { tag: "f32", value: node.val };
                break;
            case "f64-value":
                value = { tag: "f64", value: node.val };
                break;
            case "char-value":
                value = { tag: "char", value: node.val };
                break;
            case "string-value":
                value = { tag: "string", value: node.val };
                break;
            case "record-value":
                value = { tag: "record", fields: node.val.map(child) };
                break;
            case "variant-value":
                value = {
                    tag: "variant",
                    caseIndex: node.val.case_,
                    payload: node.val.payload === undefined ? undefined : child(node.val.payload),
                };
                break;
            case "enum-value":
                value = { tag: "enum", caseIndex: node.val };
                break;
            case "flags-value":
                value = { tag: "flags", flags: node.val };
                break;
            case "tuple-value":
                value = { tag: "tuple", elements: node.val.map(child) };
                break;
            case "list-value":
                value = { tag: "list", elements: node.val.map(child) };
                break;
            case "fixed-list-value":
                value = { tag: "fixed-list", elements: node.val.map(child) };
                break;
            case "map-value":
                value = {
                    tag: "map",
                    entries: node.val.map((entry) => ({ key: child(entry.key), value: child(entry.value) })),
                };
                break;
            case "option-value":
                value = { tag: "option", value: node.val === undefined ? undefined : child(node.val) };
                break;
            case "result-value":
                value = {
                    tag: "result",
                    result: {
                        tag: node.val.tag === "ok-value" ? "ok" : "err",
                        value: node.val.val === undefined ? undefined : child(node.val.val),
                    },
                };
                break;
            case "text-value":
                value = { tag: "text", text: node.val.text, language: node.val.language };
                break;
            case "binary-value":
                value = { tag: "binary", bytes: node.val.bytes, mimeType: node.val.mimeType };
                break;
            case "path-value":
                value = { tag: "path", value: node.val };
                break;
            case "url-value":
                value = { tag: "url", value: node.val };
                break;
            case "uuid-value":
                value = { tag: "uuid", value: node.val };
                break;
            case "datetime-value":
                value = { tag: "datetime", value: node.val };
                break;
            case "duration-value":
                value = { tag: "duration", nanoseconds: node.val.nanoseconds };
                break;
            case "quantity-value-node":
                value = { tag: "quantity", value: node.val };
                break;
            case "union-value":
                value = { tag: "union", unionTag: node.val.tag, body: child(node.val.body) };
                break;
            case "secret-value":
                if (node.val === undefined || capabilities.has(node.val))
                    throw new TypeError("invalid secret");
                capabilities.add(node.val);
                value = { tag: "secret", handle: null };
                break;
            case "quota-token-handle":
                if (node.val === undefined || capabilities.has(node.val))
                    throw new TypeError("invalid quota token");
                capabilities.add(node.val);
                value = { tag: "quota-token", handle: null };
                break;
            case "permission-card-handle":
                if (node.val === undefined || capabilities.has(node.val))
                    throw new TypeError("invalid permission card");
                capabilities.add(node.val);
                value = { tag: "permission-card", handle: null };
                break;
            case "stream-value":
                if (node.val === undefined || capabilities.has(node.val))
                    throw new TypeError("invalid stream");
                capabilities.add(node.val);
                value = { tag: "stream", handle: null };
                break;
        }
        onPath.delete(index);
        return value;
    };
    try {
        const value = decode(tree.root);
        return reached.size === tree.valueNodes.length && schemaValueMatches(graph, type, value);
    }
    catch {
        return false;
    }
}
function schemaValueConforms(graph, type, value) {
    try {
        assertSchemaValueRepresentable(value);
    }
    catch {
        return false;
    }
    return schemaValueMatches(graph, type, value);
}
function schemaValueMatches(graph, type, value) {
    const body = resolveType(graph, type, new Set());
    if (!body)
        return false;
    const resolvedBody = body.body;
    switch (resolvedBody.tag) {
        case "bool":
            return value.tag === "bool" && typeof value.value === "boolean";
        case "s8":
            return (value.tag === "s8" &&
                integerInRange(value.value, -128n, 127n) &&
                numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value)));
        case "s16":
            return (value.tag === "s16" &&
                integerInRange(value.value, -32768n, 32767n) &&
                numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value)));
        case "s32":
            return (value.tag === "s32" &&
                integerInRange(value.value, -(2n ** 31n), 2n ** 31n - 1n) &&
                numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value)));
        case "s64":
            return (value.tag === "s64" &&
                typeof value.value === "bigint" &&
                value.value >= -(2n ** 63n) &&
                value.value <= 2n ** 63n - 1n &&
                numericRestrictionsMatch(resolvedBody.restrictions, value.value));
        case "u8":
            return (value.tag === "u8" &&
                integerInRange(value.value, 0n, 255n) &&
                numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value)));
        case "u16":
            return (value.tag === "u16" &&
                integerInRange(value.value, 0n, 65535n) &&
                numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value)));
        case "u32":
            return (value.tag === "u32" &&
                integerInRange(value.value, 0n, 2n ** 32n - 1n) &&
                numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value)));
        case "u64":
            return (value.tag === "u64" &&
                typeof value.value === "bigint" &&
                value.value >= 0n &&
                value.value <= 2n ** 64n - 1n &&
                numericRestrictionsMatch(resolvedBody.restrictions, value.value));
        case "f32":
            return (value.tag === "f32" &&
                typeof value.value === "number" &&
                numericRestrictionsMatch(resolvedBody.restrictions, Math.fround(value.value)));
        case "f64":
            return (value.tag === "f64" &&
                typeof value.value === "number" &&
                numericRestrictionsMatch(resolvedBody.restrictions, value.value));
        case "char":
            return value.tag === "char" && isUnicodeScalar(value.value);
        case "string":
            return value.tag === "string" && typeof value.value === "string";
        case "path":
            return (value.tag === "path" &&
                typeof value.value === "string" &&
                pathMatches(resolvedBody.spec.allowedExtensions, value.value));
        case "url":
            return (value.tag === "url" &&
                typeof value.value === "string" &&
                urlMatches(resolvedBody.restrictions, value.value));
        case "uuid":
            return (value.tag === "uuid" &&
                typeof value.value.highBits === "bigint" &&
                value.value.highBits >= 0n &&
                value.value.highBits <= 2n ** 64n - 1n &&
                typeof value.value.lowBits === "bigint" &&
                value.value.lowBits >= 0n &&
                value.value.lowBits <= 2n ** 64n - 1n);
        case "datetime":
            return (value.tag === "datetime" &&
                typeof value.value.seconds === "bigint" &&
                value.value.seconds >= -(2n ** 63n) &&
                value.value.seconds <= 2n ** 63n - 1n &&
                Number.isInteger(value.value.nanoseconds) &&
                value.value.nanoseconds >= 0 &&
                value.value.nanoseconds < 1_000_000_000);
        case "duration":
            return (value.tag === "duration" &&
                typeof value.nanoseconds === "bigint" &&
                value.nanoseconds >= -(2n ** 63n) &&
                value.nanoseconds <= 2n ** 63n - 1n);
        case "secret":
        case "quota-token":
            return value.tag === resolvedBody.tag;
        case "permission-card": {
            if (value.tag !== "permission-card")
                return false;
            // A received card whose kind is not known yet matches either flag.
            const kind = isGuestPermissionCardHandle(value.handle)
                ? guestPermissionCardHandleKind(value.handle)
                : undefined;
            return kind === undefined || (kind === "template") === resolvedBody.spec.polymorphic;
        }
        case "text": {
            if (value.tag !== "text")
                return false;
            if (typeof value.text !== "string" ||
                (value.language !== undefined && typeof value.language !== "string")) {
                return false;
            }
            const length = [...value.text].length;
            return ((value.language === undefined ||
                resolvedBody.restrictions.languages === undefined ||
                resolvedBody.restrictions.languages.includes(value.language)) &&
                (resolvedBody.restrictions.minLength === undefined ||
                    length >= resolvedBody.restrictions.minLength) &&
                (resolvedBody.restrictions.maxLength === undefined ||
                    length <= resolvedBody.restrictions.maxLength) &&
                (resolvedBody.restrictions.regex === undefined ||
                    regexMatches(resolvedBody.restrictions.regex, value.text)));
        }
        case "binary":
            return (value.tag === "binary" &&
                value.bytes instanceof Uint8Array &&
                (value.mimeType === undefined || typeof value.mimeType === "string") &&
                (value.mimeType === undefined ||
                    resolvedBody.restrictions.mimeTypes === undefined ||
                    resolvedBody.restrictions.mimeTypes.includes(value.mimeType)) &&
                (resolvedBody.restrictions.minBytes === undefined ||
                    value.bytes.byteLength >= resolvedBody.restrictions.minBytes) &&
                (resolvedBody.restrictions.maxBytes === undefined ||
                    value.bytes.byteLength <= resolvedBody.restrictions.maxBytes));
        case "quantity":
            return value.tag === "quantity" && quantityMatches(resolvedBody.spec, value.value);
        case "record":
            return (value.tag === "record" &&
                value.fields.length === resolvedBody.fields.length &&
                resolvedBody.fields.every((entry, index) => schemaValueMatches(graph, entry.body, value.fields[index])));
        case "variant": {
            if (value.tag !== "variant")
                return false;
            const selected = resolvedBody.cases[value.caseIndex];
            return (!!selected &&
                (selected.payload === undefined
                    ? value.payload === undefined
                    : value.payload !== undefined &&
                        schemaValueMatches(graph, selected.payload, value.payload)));
        }
        case "enum":
            return (value.tag === "enum" && value.caseIndex >= 0 && value.caseIndex < resolvedBody.cases.length);
        case "flags":
            return (value.tag === "flags" &&
                Array.isArray(value.flags) &&
                value.flags.length === resolvedBody.names.length &&
                value.flags.every((flag) => typeof flag === "boolean"));
        case "tuple":
            return (value.tag === "tuple" &&
                value.elements.length === resolvedBody.elements.length &&
                resolvedBody.elements.every((entry, index) => schemaValueMatches(graph, entry, value.elements[index])));
        case "list":
            return (value.tag === "list" &&
                value.elements.every((entry) => schemaValueMatches(graph, resolvedBody.element, entry)));
        case "fixed-list":
            return (value.tag === "fixed-list" &&
                value.elements.length === resolvedBody.length &&
                value.elements.every((entry) => schemaValueMatches(graph, resolvedBody.element, entry)));
        case "map":
            return (value.tag === "map" &&
                value.entries.every((entry) => schemaValueMatches(graph, resolvedBody.key, entry.key) &&
                    schemaValueMatches(graph, resolvedBody.value, entry.value)));
        case "option":
            return (value.tag === "option" &&
                (value.value === undefined || schemaValueMatches(graph, resolvedBody.element, value.value)));
        case "result":
            return (value.tag === "result" &&
                (value.result.tag === "ok"
                    ? resolvedBody.ok === undefined
                        ? value.result.value === undefined
                        : value.result.value !== undefined &&
                            schemaValueMatches(graph, resolvedBody.ok, value.result.value)
                    : resolvedBody.err === undefined
                        ? value.result.value === undefined
                        : value.result.value !== undefined &&
                            schemaValueMatches(graph, resolvedBody.err, value.result.value)));
        case "union": {
            if (value.tag !== "union")
                return false;
            const branch = resolvedBody.branches.find((entry) => entry.tag === value.unionTag);
            return (!!branch &&
                schemaValueMatches(graph, branch.body, value.body) &&
                discriminatorMatches$1(graph, branch, value.body));
        }
        case "stream":
            return value.tag === "stream";
        case "future":
        case "ref":
            return false;
    }
}
function discriminatorMatches$1(graph, branch, value) {
    const discriminator = branch.discriminator;
    switch (discriminator.tag) {
        case "prefix":
            return stringView(value)?.startsWith(discriminator.val) ?? false;
        case "suffix":
            return stringView(value)?.endsWith(discriminator.val) ?? false;
        case "contains":
            return stringView(value)?.includes(discriminator.val) ?? false;
        case "regex": {
            const text = stringView(value);
            return text !== undefined && regexMatches(discriminator.val, text);
        }
        case "field-equals": {
            const record = recordView(graph, branch.body, value);
            if (!record)
                return false;
            const index = record.names.indexOf(discriminator.val.fieldName);
            if (index < 0)
                return false;
            return (discriminator.val.literal === undefined ||
                stringView(record.values[index]) === discriminator.val.literal);
        }
        case "field-absent": {
            const record = recordView(graph, branch.body, value);
            return !!record && !record.names.includes(discriminator.val);
        }
    }
}
function stringView(value) {
    switch (value.tag) {
        case "string":
        case "path":
        case "url":
            return value.value;
        case "text":
            return value.text;
        default:
            return undefined;
    }
}
function recordView(graph, type, value) {
    const resolved = resolveType(graph, type, new Set());
    return resolved?.body.tag === "record" &&
        value.tag === "record" &&
        value.fields.length === resolved.body.fields.length
        ? { names: resolved.body.fields.map((field) => field.name), values: value.fields }
        : undefined;
}
function resolveType(graph, type, visited) {
    if (type.body.tag !== "ref")
        return type;
    if (visited.has(type.body.id))
        return undefined;
    visited.add(type.body.id);
    const definition = graph.defs.get(type.body.id);
    return definition ? resolveType(graph, definition.body, visited) : undefined;
}
function integerInRange(value, min, max) {
    return Number.isInteger(value) && BigInt(value) >= min && BigInt(value) <= max;
}
function numericRestrictionsMatch(restrictions, value) {
    if (!restrictions)
        return true;
    const compare = (bound) => {
        if (typeof value === "number") {
            if (bound.tag !== "float-bits")
                return undefined;
            const decoded = floatFromBits(bound.val);
            if (decoded === undefined || !Number.isFinite(decoded) || Number.isNaN(value)) {
                return undefined;
            }
            return value < decoded ? -1 : value > decoded ? 1 : 0;
        }
        if (bound.tag === "float-bits")
            return undefined;
        return value < bound.val ? -1 : value > bound.val ? 1 : 0;
    };
    const min = restrictions.min ? compare(restrictions.min) : 0;
    const max = restrictions.max ? compare(restrictions.max) : 0;
    return min !== undefined && max !== undefined && min >= 0 && max <= 0;
}
function isUnicodeScalar(value) {
    const points = [...value];
    if (points.length !== 1)
        return false;
    const codePoint = points[0].codePointAt(0);
    return codePoint !== undefined && (codePoint < 0xd800 || codePoint > 0xdfff);
}
function regexMatches(pattern, value) {
    try {
        return new RegExp(pattern, "u").test(value);
    }
    catch {
        return false;
    }
}
function pathMatches(allowedExtensions, value) {
    if (value.length === 0)
        return false;
    if (!allowedExtensions)
        return true;
    const name = value.split("/").at(-1);
    const dot = name?.lastIndexOf(".") ?? -1;
    if (dot < 0 || dot + 1 >= (name?.length ?? 0))
        return true;
    return allowedExtensions.includes(name.slice(dot + 1));
}
function urlMatches(restrictions, value) {
    try {
        const url = new URL(value);
        return ((restrictions.allowedSchemes === undefined ||
            restrictions.allowedSchemes.some((scheme) => scheme.toLowerCase() === url.protocol.slice(0, -1).toLowerCase())) &&
            (restrictions.allowedHosts === undefined ||
                restrictions.allowedHosts.some((host) => host.toLowerCase() === url.hostname.toLowerCase())));
    }
    catch {
        return false;
    }
}
function quantityMatches(spec, value) {
    if (!isQuantityValueRepresentable(value))
        return false;
    const unitAllowed = spec.allowedSuffixes.length === 0
        ? value.unit === spec.baseUnit
        : spec.allowedSuffixes.includes(value.unit);
    return (unitAllowed &&
        (spec.min === undefined || quantityLessOrEqual(spec.min, value) === true) &&
        (spec.max === undefined || quantityLessOrEqual(value, spec.max) === true));
}

/** A typed immutable facade over one root in a reflected schema graph. @since 1.6.0 @category models */
class SchemaRef {
    graph;
    root;
    constructor(wireGraph, root = wireGraph.root) {
        const decoded = schemaGraphFromWit({ ...wireGraph, root });
        this.graph = freezeSchemaGraph(decoded);
        this.root = this.graph.root;
        Object.freeze(this);
    }
    /** Build a view over an already decoded, deeply immutable graph. @since 1.6.0 @category constructors */
    static fromImmutableGraph(graph, root) {
        const ref = Object.create(SchemaRef.prototype);
        Object.defineProperties(ref, {
            graph: { value: Object.freeze({ defs: graph.defs, root }), enumerable: true },
            root: { value: root, enumerable: true },
        });
        return Object.freeze(ref);
    }
    /** Pack canonical JSON and validate it against the complete schema. @since 1.6.0 @category conversions */
    packJson(value) {
        const model = fromCanonicalJson(this.graph, this.root, value, [], jsonFieldDefaults(this));
        if (!schemaValueConforms(this.graph, this.root, model))
            throw new SchemaRenderError([], "schema value does not conform to the expected schema");
        return schemaValueToWit(model);
    }
    /** Unpack a native schema value into canonical JSON. @since 1.6.0 @category conversions */
    unpackJson(value) {
        return withCapabilityAdoptionTransaction((transaction) => {
            const model = schemaValueFromWit(value, transaction);
            if (!schemaValueConforms(this.graph, this.root, model))
                throw new SchemaRenderError([], "schema value does not conform to the expected schema");
            return toCanonicalJson(this.graph, this.root, model);
        });
    }
    /** Explicitly validate canonical JSON. @since 1.6.0 @category validation */
    validateJson(value) {
        try {
            return { success: true, value: this.packJson(value) };
        }
        catch (error) {
            return invalid(error);
        }
    }
    /**
     * Validate WIT membership and restrictions, independently of JSON representability.
     * Unrestricted native floats include NaN and infinities; native integers may exceed
     * the safe JSON range. Use validateJson or unpackJson for canonical JSON boundaries.
     * @since 1.6.0 @category validation
     */
    validateValue(value) {
        return schemaValueTreeConforms(this.graph, this.root, value)
            ? { success: true, value }
            : invalid("schema value does not conform to the expected schema");
    }
    /** Check whether this root has an unambiguous canonical JSON representation. @since 1.6.0 @category validation */
    jsonEligibility() {
        try {
            assertCanonicalJsonEligible(this.graph, this.root);
            return { success: true, value: undefined };
        }
        catch (error) {
            return invalid(error);
        }
    }
    /** Render this root as JSON Schema. @since 1.6.0 @category conversions */
    toJsonSchema(options = {}) {
        const rendered = toCanonicalJsonSchema(this.graph, this.root, false, jsonFieldDefaults(this));
        return options.includeDraftMarker === false
            ? rendered
            : { $schema: "https://json-schema.org/draft/2020-12/schema", ...rendered };
    }
}
const invalid = (error) => ({
    success: false,
    issues: [
        {
            path: error instanceof SchemaRenderError ? error.path : [],
            message: error instanceof Error ? error.message : String(error),
        },
    ],
});
/** @internal Freeze a decoded schema graph including its definition map. */
const freezeSchemaGraph = (value, seen = new WeakSet()) => {
    if (value === null || typeof value !== "object" || seen.has(value))
        return value;
    if (value instanceof Uint8Array)
        return value;
    seen.add(value);
    if (value instanceof Map) {
        for (const [key, item] of value) {
            freezeSchemaGraph(key, seen);
            freezeSchemaGraph(item, seen);
        }
        Object.defineProperties(value, {
            set: {
                value: () => {
                    throw new TypeError("SchemaRef maps are immutable");
                },
            },
            delete: {
                value: () => {
                    throw new TypeError("SchemaRef maps are immutable");
                },
            },
            clear: {
                value: () => {
                    throw new TypeError("SchemaRef maps are immutable");
                },
            },
        });
    }
    else
        for (const item of Object.values(value))
            freezeSchemaGraph(item, seen);
    return Object.freeze(value);
};

/**
 * @since 1.6.0
 *
 * Compiles an Effect Schema into the `golem:core/types@2.0.0` schema model:
 * a recursive {@link SchemaType} (the WIT type) plus a bidirectional value
 * codec to/from {@link SchemaValue}. The flat wire carriers
 * (`schema-graph` / `schema-value-tree`) are produced from these by
 * `./internal/schema-model/wit.ts` at the dispatch boundary.
 */
// Branded so `Durability.wrap` (and any other downstream consumer that uses
// nominal SDK-error detection) can route this into the defect channel without
// `_tag`-string sniffing. `Symbol.for(...)` guarantees the same runtime symbol
// across modules.
const sdkErrorBrand = Symbol.for("effect-golem/durable-function/sdk-error");
/**
 * Raised by {@link toWitCodec} (and registration helpers that compile a user
 * schema) when an Effect Schema construct cannot be represented in the Golem
 * schema model.
 *
 * @since 1.6.0
 * @category errors
 */
class UnsupportedSchemaError {
    reason;
    _tag = "UnsupportedSchemaError";
    [sdkErrorBrand] = true;
    constructor(reason) {
        this.reason = reason;
    }
}
class WireWriter {
    nodes = [];
    add(node) {
        this.nodes.push(node);
        return this.nodes.length - 1;
    }
}
const wireCodecs = new WeakMap();
const sequenceWire = (tag, pairs, length) => pairs.every((p) => p.wire)
    ? {
        write: (xs, writer) => {
            if (tag === "fixed-list-value" && xs.length !== length)
                throw new Error(`value: expected ${tag} with ${length} elements`);
            return writer.add({
                tag,
                val: Array.from({ length }, (_, i) => pairs[i % pairs.length].wire.write(xs[i], writer)),
            });
        },
        read: (nodes, index) => {
            const indices = wireNode(nodes, index, tag).val;
            if (indices.length !== length)
                throw new Error(`value: expected ${tag} with ${length} elements`);
            return indices.map((child, i) => pairs[i % pairs.length].wire.read(nodes, child));
        },
    }
    : undefined;
const mapWire = (key, value, native, hash = false) => key.wire && value.wire
    ? {
        write: (map, writer) => {
            const entries = hash ? HashMap.toEntries(map) : map.entries();
            if (native)
                return writer.add({
                    tag: "map-value",
                    val: Array.from(entries, ([k, v]) => ({
                        key: key.wire.write(k, writer),
                        value: value.wire.write(v, writer),
                    })),
                });
            return writer.add({
                tag: "list-value",
                val: Array.from(entries, ([k, v]) => writer.add({
                    tag: "tuple-value",
                    val: [key.wire.write(k, writer), value.wire.write(v, writer)],
                })),
            });
        },
        read: (nodes, index) => {
            const entries = native
                ? wireNode(nodes, index, "map-value").val.map((e) => [
                    key.wire.read(nodes, e.key),
                    value.wire.read(nodes, e.value),
                ])
                : wireNode(nodes, index, "list-value").val.map((i) => {
                    const tuple = wireNode(nodes, i, "tuple-value").val;
                    if (tuple.length !== 2)
                        throw new Error("value: expected tuple with 2 elements");
                    return [key.wire.read(nodes, tuple[0]), value.wire.read(nodes, tuple[1])];
                });
            return hash ? HashMap.fromIterable(entries) : new Map(entries);
        },
    }
    : undefined;
const wireNode = (nodes, index, tag) => {
    const node = nodes[index];
    if (node.tag !== tag)
        throw new Error(`value: expected ${tag}, received ${node.tag}`);
    return node;
};
let conversionContext;
const withConversionContext = (context, f) => {
    const previous = conversionContext;
    conversionContext = context;
    try {
        return f();
    }
    finally {
        conversionContext = previous;
    }
};
const requireConversionContext = () => {
    if (conversionContext === undefined)
        throw new Error("stream conversion requires an Effect context");
    return conversionContext;
};
/** Leaf pair for a primitive whose schema value carries a single `value`. */
const primPair = (make, numericTag, coerce = (val) => val) => {
    const tag = numericTag ??
        (make === v.string
            ? "string"
            : make === v.bool
                ? "bool"
                : make === v.char
                    ? "char"
                    : make === v.f64
                        ? "f64"
                        : make === v.s64
                            ? "s64"
                            : make === v.path
                                ? "path"
                                : make === v.url
                                    ? "url"
                                    : make === v.uuid
                                        ? "uuid"
                                        : make === v.datetime
                                            ? "datetime"
                                            : make === v.quantity
                                                ? "quantity"
                                                : undefined);
    const wireTag = (tag === "quantity" ? "quantity-value-node" : `${tag}-value`);
    return {
        toValue: (val) => make(val),
        fromValue: (sv) => sv.value,
        ...(tag === undefined
            ? {}
            : {
                wire: {
                    write: (val, writer) => {
                        return writer.add({
                            tag: wireTag,
                            val: coerce(val),
                        });
                    },
                    read: (nodes, index) => wireNode(nodes, index, wireTag).val,
                },
            }),
    };
};
const assertValueShape = (graph, type, value, path = "value") => {
    const body = type.body.tag === "ref" ? graph.defs.get(type.body.id)?.body.body : type.body;
    if (body === undefined)
        throw new Error(`${path}: unresolved schema reference`);
    const expectedTag = body.tag === "fixed-list" ? "fixed-list" : body.tag;
    if (value.tag !== expectedTag) {
        throw new Error(`${path}: expected ${expectedTag}, received ${value.tag}`);
    }
    switch (body.tag) {
        case "record":
            if (value.tag !== "record" || value.fields.length !== body.fields.length) {
                throw new Error(`${path}: expected record with ${body.fields.length} fields`);
            }
            body.fields.forEach((field, i) => assertValueShape(graph, field.body, value.fields[i], `${path}.${field.name}`));
            break;
        case "tuple":
            if (value.tag !== "tuple" || value.elements.length !== body.elements.length) {
                throw new Error(`${path}: expected tuple with ${body.elements.length} elements`);
            }
            body.elements.forEach((element, i) => assertValueShape(graph, element, value.elements[i], `${path}[${i}]`));
            break;
        case "list":
            if (value.tag === "list") {
                value.elements.forEach((element, i) => assertValueShape(graph, body.element, element, `${path}[${i}]`));
            }
            break;
        case "fixed-list":
            if (value.tag !== "fixed-list" || value.elements.length !== body.length) {
                throw new Error(`${path}: expected fixed-list with ${body.length} elements`);
            }
            value.elements.forEach((element, i) => assertValueShape(graph, body.element, element, `${path}[${i}]`));
            break;
        case "option":
            if (value.tag === "option" && value.value !== undefined) {
                assertValueShape(graph, body.element, value.value, `${path}.some`);
            }
            break;
        case "variant": {
            if (value.tag !== "variant")
                break;
            const variant = body.cases[value.caseIndex];
            if (variant === undefined)
                throw new Error(`${path}: invalid variant case ${value.caseIndex}`);
            if ((variant.payload === undefined) !== (value.payload === undefined)) {
                throw new Error(`${path}: variant payload arity mismatch`);
            }
            if (variant.payload !== undefined && value.payload !== undefined) {
                assertValueShape(graph, variant.payload, value.payload, `${path}.${variant.name}`);
            }
            break;
        }
        case "enum":
            if (value.tag === "enum" && body.cases[value.caseIndex] === undefined) {
                throw new Error(`${path}: invalid enum case ${value.caseIndex}`);
            }
            break;
        case "flags":
            if (value.tag !== "flags" || value.flags.length !== body.names.length) {
                throw new Error(`${path}: expected flags with ${body.names.length} values`);
            }
            break;
        case "union": {
            if (value.tag !== "union")
                break;
            const branch = body.branches.find((candidate) => candidate.tag === value.unionTag);
            if (branch === undefined)
                throw new Error(`${path}: unknown union branch '${value.unionTag}'`);
            assertValueShape(graph, branch.body, value.body, `${path}.${branch.tag}`);
            break;
        }
    }
};
/**
 * Mapping from a {@link WitNumericKind} annotation to the matching schema-type
 * constructor, schema-value constructor, and a coercion re-shaping the encoded
 * JS value into what the value expects (e.g. `bigint` for `u64`/`s64`).
 */
const numericMapping = {
    u8: { make: t.u8, toV: v.u8, coerce: (v) => v },
    u16: { make: t.u16, toV: v.u16, coerce: (v) => v },
    u32: { make: t.u32, toV: v.u32, coerce: (v) => v },
    u64: {
        make: t.u64,
        toV: v.u64,
        coerce: (v) => (typeof v === "bigint" ? v : BigInt(v)),
    },
    s8: { make: t.s8, toV: v.s8, coerce: (v) => v },
    s16: { make: t.s16, toV: v.s16, coerce: (v) => v },
    s32: { make: t.s32, toV: v.s32, coerce: (v) => v },
    s64: {
        make: t.s64,
        toV: v.s64,
        coerce: (v) => (typeof v === "bigint" ? v : BigInt(v)),
    },
    f32: { make: t.f32, toV: v.f32, coerce: (v) => v },
    f64: { make: t.f64, toV: v.f64, coerce: (v) => v },
};
/**
 * Look up an annotation by key on an AST node. Effect Schema attaches
 * annotations from `Schema.annotate(...)` to the *last check* (refinement),
 * not the AST root, so we delegate to `SchemaAST.resolveAt` which knows the
 * right traversal order.
 */
const annotationOf = (a, key) => SchemaAST.resolveAt(key)(a);
const metadataOf = (a) => {
    const annotations = (a.annotations ?? {});
    const explicit = annotationOf(a, witMetadataAnnotationKey) ?? {};
    const examples = explicit.examples ??
        (Array.isArray(annotations.examples)
            ? annotations.examples.map((example) => JSON.stringify(example))
            : []);
    return {
        ...emptyMetadata(),
        ...(typeof annotations.description === "string" ? { doc: annotations.description } : {}),
        ...(typeof annotations.deprecated === "string" ? { deprecated: annotations.deprecated } : {}),
        ...explicit,
        aliases: explicit.aliases ? [...explicit.aliases] : [],
        examples: [...examples],
    };
};
const numericKindOf = (a) => annotationOf(a, witTypeAnnotationKey);
const F64_BITS_VIEW = new DataView(new ArrayBuffer(8));
const f64Bits = (x) => {
    // Canonicalize -0.0 to +0.0 so equal bounds compare equal (mirrors the codec).
    F64_BITS_VIEW.setFloat64(0, x === 0 ? 0 : x);
    return F64_BITS_VIEW.getBigUint64(0);
};
/** The `numeric-bound` tag for a numeric pin kind. */
const boundKindOf = (kind) => kind[0] === "f" ? "float-bits" : kind[0] === "s" ? "signed" : "unsigned";
const makeBound = (boundKind, x) => boundKind === "float-bits"
    ? { tag: "float-bits", val: f64Bits(Number(x)) }
    : { tag: boundKind, val: BigInt(x) };
/** Read inline numeric restrictions (from a `restrict(...)` annotation) for `kind`. */
const numericRestrictionsOf = (a, kind) => {
    const opts = annotationOf(a, witNumericRestrictionsKey);
    if (!opts || (opts.min === undefined && opts.max === undefined && !opts.unit))
        return undefined;
    const bk = boundKindOf(kind);
    return {
        min: opts.min !== undefined ? makeBound(bk, opts.min) : undefined,
        max: opts.max !== undefined ? makeBound(bk, opts.max) : undefined,
        unit: opts.unit,
    };
};
const numericNode = (kind, a) => {
    const m = numericMapping[kind];
    return {
        type: m.make(numericRestrictionsOf(a, kind)),
        pair: primPair((val) => m.toV(m.coerce(val)), kind, m.coerce),
    };
};
const isNullOrUndefinedAST = (a) => a._tag === "Null" || a._tag === "Undefined" || a._tag === "Void";
const isVoidLikeAST = (a) => a._tag === "Undefined" || a._tag === "Void";
/**
 * Compute an `EncodedShape` for a schema AST. Operates on the *encoded* form
 * because the codec walks encoded values, not decoded ones.
 */
const encodedShapeOf = (a) => {
    switch (a._tag) {
        case "String":
            return { tag: "string", matches: (v) => typeof v === "string" };
        case "Number":
            return { tag: "number", matches: (v) => typeof v === "number" };
        case "Boolean":
            return { tag: "boolean", matches: (v) => typeof v === "boolean" };
        case "BigInt":
            return { tag: "bigint", matches: (v) => typeof v === "bigint" };
        case "Null":
            return { tag: "null", matches: (v) => v === null };
        case "Undefined":
        case "Void":
            return { tag: "undefined", matches: (v) => v === undefined };
        case "Literal": {
            const lit = a.literal;
            return {
                tag: "literal",
                literal: lit,
                matches: (v) => v === lit,
            };
        }
        case "Arrays":
            return { tag: "array", matches: Array.isArray };
        case "Enum": {
            const values = a.enums.map(([, value]) => value);
            const kind = typeof values[0];
            return {
                tag: kind === "string" ? "string" : "number",
                matches: (value) => typeof value === kind && values.includes(value),
            };
        }
        case "Objects": {
            const tagPs = a.propertySignatures.find((ps) => ps.name === "_tag");
            // Only treat _tag as a discriminator if the property is *required*.
            if (tagPs !== undefined &&
                !SchemaAST.isOptional(tagPs.type) &&
                SchemaAST.isLiteral(tagPs.type) &&
                typeof tagPs.type.literal === "string") {
                const lit = tagPs.type.literal;
                return {
                    tag: "object-with-tag",
                    tagLiteral: lit,
                    matches: (v) => typeof v === "object" &&
                        v !== null &&
                        !Array.isArray(v) &&
                        v._tag === lit,
                };
            }
            return {
                tag: "object",
                matches: (v) => typeof v === "object" && v !== null && !Array.isArray(v),
            };
        }
        default:
            return { tag: "unknown", matches: () => true };
    }
};
const variantCaseNameOf = (a) => annotationOf(a, variantCaseNameAnnotationKey);
const discriminatorMatches = (rule, value) => {
    switch (rule.tag) {
        case "prefix":
            return typeof value === "string" && value.startsWith(rule.val);
        case "suffix":
            return typeof value === "string" && value.endsWith(rule.val);
        case "contains":
            return typeof value === "string" && value.includes(rule.val);
        case "regex":
            return typeof value === "string" && new RegExp(rule.val, "u").test(value);
        case "field-equals":
            return (typeof value === "object" &&
                value !== null &&
                rule.val.fieldName in value &&
                (rule.val.literal === undefined ||
                    value[rule.val.fieldName] === rule.val.literal));
        case "field-absent":
            return typeof value === "object" && value !== null && !(rule.val in value);
    }
};
const declarationConstructorTag = (a) => {
    if (a._tag !== "Declaration")
        return undefined;
    const id = a.annotations?.representation?.id;
    switch (id) {
        case "effect/schema/Option":
            return "effect/Option";
        case "effect/schema/Result":
            return "effect/Result";
        case "effect/schema/ReadonlyMap":
            return "ReadonlyMap";
        case "effect/schema/HashMap":
            return "effect/HashMap";
        default:
            return undefined;
    }
};
const typedArrayKindOf = (a) => annotationOf(a, witTypedArrayAnnotationKey);
const isQuotaTokenAST = (a) => annotationOf(a, witQuotaTokenAnnotationKey) === true;
const quotaTokenResourceNameOf = (a) => annotationOf(a, WIT_QUOTA_TOKEN_RESOURCE_NAME_ANNOTATION_KEY);
const isPrincipalAST = (a) => annotationOf(a, witPrincipalAnnotationKey) === true;
// --- graph type builders (no recursion: everything is built inline) ---
const uuidType = () => t.uuid();
const componentIdType = () => t.record([field("uuid", uuidType())]);
const agentIdType = () => t.record([field("componentId", componentIdType()), field("agentId", t.string())]);
const accountIdType = () => t.record([field("uuid", uuidType())]);
const oidcType = () => t.record([
    field("sub", t.string()),
    field("issuer", t.string()),
    field("email", t.option(t.string())),
    field("name", t.option(t.string())),
    field("emailVerified", t.option(t.bool())),
    field("givenName", t.option(t.string())),
    field("familyName", t.option(t.string())),
    field("picture", t.option(t.string())),
    field("preferredUsername", t.option(t.string())),
    field("claims", t.string()),
]);
const agentPrincipalType = () => t.record([field("agentId", agentIdType())]);
const golemUserType = () => t.record([field("accountId", accountIdType())]);
// --- SchemaValue field accessors (positional record reads) ---
const recFields = (sv) => sv.fields;
const strOf = (f) => f.value;
const boolOf = (f) => f.value;
const optOf = (f) => f.value;
// --- codec helpers (round-trip via the host shape) ---
const uuidToValue = (u) => v.uuid(u);
const uuidFromValue = (sv) => sv.value;
const agentIdToValue = (a) => v.record([v.record([uuidToValue(a.componentId.uuid)]), v.string(a.agentId)]);
const agentIdFromValue = (sv) => {
    const f = recFields(sv);
    const uuid = uuidFromValue(recFields(f[0])[0]);
    return { componentId: { uuid }, agentId: strOf(f[1]) };
};
const accountIdToValue = (a) => v.record([uuidToValue(a.uuid)]);
const accountIdFromValue = (sv) => ({
    uuid: uuidFromValue(recFields(sv)[0]),
});
const oidcToValue = (o) => {
    const optStr = (x) => v.option(x === undefined ? undefined : v.string(x));
    return v.record([
        v.string(o.sub),
        v.string(o.issuer),
        optStr(o.email),
        optStr(o.name),
        v.option(o.emailVerified === undefined ? undefined : v.bool(o.emailVerified)),
        optStr(o.givenName),
        optStr(o.familyName),
        optStr(o.picture),
        optStr(o.preferredUsername),
        v.string(o.claims),
    ]);
};
const oidcFromValue = (sv) => {
    const f = recFields(sv);
    const optStr = (fld) => {
        const val = optOf(fld);
        return val === undefined ? undefined : strOf(val);
    };
    const out = {
        sub: strOf(f[0]),
        issuer: strOf(f[1]),
        claims: strOf(f[9]),
    };
    const email = optStr(f[2]);
    if (email !== undefined)
        out.email = email;
    const name = optStr(f[3]);
    if (name !== undefined)
        out.name = name;
    const ev = optOf(f[4]);
    if (ev !== undefined)
        out.emailVerified = boolOf(ev);
    const givenName = optStr(f[5]);
    if (givenName !== undefined)
        out.givenName = givenName;
    const familyName = optStr(f[6]);
    if (familyName !== undefined)
        out.familyName = familyName;
    const picture = optStr(f[7]);
    if (picture !== undefined)
        out.picture = picture;
    const preferredUsername = optStr(f[8]);
    if (preferredUsername !== undefined)
        out.preferredUsername = preferredUsername;
    return out;
};
/**
 * Type + value bridge for the `principal` data variant. The graph root mirrors
 * the host `Principal` shape (case order oidc/agent/golem-user/anonymous); the
 * value pair lowers a host `Principal` into the matching `v.variant(...)` and
 * lifts it back out.
 */
const principalNode = () => ({
    type: t.variant([
        variantCase("oidc", oidcType()),
        variantCase("agent", agentPrincipalType()),
        variantCase("golem-user", golemUserType()),
        variantCase("anonymous"),
    ]),
    pair: {
        toValue: (p) => {
            switch (p.tag) {
                case "oidc":
                    return v.variant(0, oidcToValue(p.val));
                case "agent":
                    return v.variant(1, v.record([agentIdToValue(p.val.agentId)]));
                case "golem-user":
                    return v.variant(2, v.record([accountIdToValue(p.val.accountId)]));
                case "anonymous":
                    return v.variant(3);
            }
        },
        fromValue: (sv) => {
            const vv = sv;
            switch (vv.caseIndex) {
                case 0:
                    return { tag: "oidc", val: oidcFromValue(vv.payload) };
                case 1:
                    return {
                        tag: "agent",
                        val: { agentId: agentIdFromValue(recFields(vv.payload)[0]) },
                    };
                case 2:
                    return {
                        tag: "golem-user",
                        val: { accountId: accountIdFromValue(recFields(vv.payload)[0]) },
                    };
                default:
                    return { tag: "anonymous" };
            }
        },
    },
});
/**
 * Type + value bridge for the opaque `quota-token` capability node. The graph
 * root is `t.quotaToken({})`; the value pair lowers a host `QuotaToken` (a raw
 * owned `own<quota-token>` resource) into `v.quotaToken(handle)` and lifts it
 * back out via `handle.take()`. The take-once cell guarantees the owned handle
 * is moved exactly once; decoding a value whose handle was already consumed
 * throws.
 */
const quotaTokenNode = (resourceName) => ({
    type: t.quotaToken({ resourceName }),
    pair: {
        toValue: (handle) => {
            if (!isGuestQuotaTokenHandle(handle)) {
                throw new Error("quota-token schemas only accept SDK-owned quota tokens");
            }
            return v.quotaToken(handle);
        },
        fromValue: (sv) => {
            const handle = sv.handle;
            const raw = peekGuestQuotaTokenHandle(QUOTA_INTERNAL, handle);
            if (raw === undefined) {
                throw new Error("quota-token handle was already consumed; an owned quota-token can only be decoded once");
            }
            return handle;
        },
    },
});
/**
 * Per-typed-array element schema-type/value constructors plus an optional
 * coercion (used to keep `bigint` payloads for s64/u64 arrays without forcing
 * the user to pre-convert).
 */
const typedArrayElement = {
    u8: { make: t.u8, toV: v.u8, coerce: (v) => v },
    i8: { make: t.s8, toV: v.s8, coerce: (v) => v },
    u16: { make: t.u16, toV: v.u16, coerce: (v) => v },
    i16: { make: t.s16, toV: v.s16, coerce: (v) => v },
    u32: { make: t.u32, toV: v.u32, coerce: (v) => v },
    i32: { make: t.s32, toV: v.s32, coerce: (v) => v },
    f32: { make: t.f32, toV: v.f32, coerce: (v) => v },
    f64: { make: t.f64, toV: v.f64, coerce: (v) => v },
    "big-i64": {
        make: t.s64,
        toV: v.s64,
        coerce: (v) => (typeof v === "bigint" ? v : BigInt(v)),
    },
    "big-u64": {
        make: t.u64,
        toV: v.u64,
        coerce: (v) => (typeof v === "bigint" ? v : BigInt(v)),
    },
};
/** Construct the JS TypedArray subclass matching a `WitTypedArrayKind`. */
const typedArrayCtor = {
    u8: Uint8Array,
    i8: Int8Array,
    u16: Uint16Array,
    i16: Int16Array,
    u32: Uint32Array,
    i32: Int32Array,
    f32: Float32Array,
    f64: Float64Array,
    "big-i64": BigInt64Array,
    "big-u64": BigUint64Array,
};
/**
 * Walk a Schema AST into a recursive {@link SchemaType} plus a top-level
 * {@link ValuePair}. Unlike the WIT-graph model, the type is recursive — the
 * flattening into the indexed `schema-graph` is done later by `GraphEncoder`.
 */
const walk = (ast) => {
    const entries = new WeakMap();
    const defs = new Map();
    let nextId = 0;
    return Effect.gen(function* () {
        const unsupported = (reason) => Effect.fail(new UnsupportedSchemaError(reason));
        /** Recurse into a child AST, returning its type + pair. */
        const child = (a) => nodeFor(a, true);
        /**
         * Build a record-type node + pair given a list of property signatures.
         * Used for both `Objects` schemas and tagged-variant payloads.
         */
        const recordNode = (props) => Effect.gen(function* () {
            const fields = [];
            for (const ps of props) {
                if (typeof ps.name !== "string") {
                    return yield* unsupported(`non-string property key: ${String(ps.name)}`);
                }
                const { type: rawType, pair: rawPair } = yield* child(ps.type);
                const optional = SchemaAST.isOptional(ps.type);
                if (optional) {
                    const pair = {
                        toValue: (val) => v.option(val === undefined ? undefined : rawPair.toValue(val)),
                        fromValue: (sv) => {
                            const ov = sv;
                            return ov.value === undefined ? undefined : rawPair.fromValue(ov.value);
                        },
                        wire: rawPair.wire && {
                            write: (val, writer) => writer.add({
                                tag: "option-value",
                                val: val === undefined ? undefined : rawPair.wire.write(val, writer),
                            }),
                            read: (nodes, index) => {
                                const child = wireNode(nodes, index, "option-value").val;
                                return child === undefined ? undefined : rawPair.wire.read(nodes, child);
                            },
                        },
                    };
                    fields.push({ name: ps.name, type: t.option(rawType), pair, optional });
                }
                else {
                    fields.push({ name: ps.name, type: rawType, pair: rawPair, optional });
                }
            }
            const type = t.record(fields.map((f) => field(f.name, f.type, f.type.metadata)));
            const pair = {
                toValue: (obj) => v.record(fields.map((f) => f.pair.toValue(obj[f.name]))),
                fromValue: (sv) => {
                    const rv = sv;
                    const out = {};
                    for (let i = 0; i < fields.length; i++) {
                        const f = fields[i];
                        const val = f.pair.fromValue(rv.fields[i]);
                        if (!f.optional || val !== undefined)
                            out[f.name] = val;
                    }
                    return out;
                },
                wire: fields.every((f) => f.pair.wire)
                    ? {
                        write: (obj, writer) => writer.add({
                            tag: "record-value",
                            val: fields.map((f) => f.pair.wire.write(obj[f.name], writer)),
                        }),
                        read: (nodes, index) => {
                            const indices = wireNode(nodes, index, "record-value").val;
                            if (indices.length !== fields.length)
                                throw new Error(`value: expected record with ${fields.length} fields`);
                            const out = {};
                            fields.forEach((f, i) => {
                                const val = f.pair.wire.read(nodes, indices[i]);
                                if (!f.optional || val !== undefined)
                                    out[f.name] = val;
                            });
                            return out;
                        },
                    }
                    : undefined,
            };
            return { type, pair };
        });
        /**
         * Build an option-type node + pair wrapping an inner type/pair, with a
         * configurable "encoded empty" representation (the value standing in for
         * `None` in the user's encoded form, e.g. `null` for `NullOr`).
         */
        const optionWrap = (innerType, innerPair, empty) => {
            const isEmpty = (val) => {
                switch (empty.kind) {
                    case "null":
                        return val === null;
                    case "undefined":
                        return val === undefined;
                    case "effect-option":
                        return Option.isOption(val) && Option.isNone(val);
                }
            };
            const wrap = (encoded) => empty.kind === "effect-option" ? Option.some(encoded) : encoded;
            const unwrap = (val) => empty.kind === "effect-option"
                ? val.pipe(Option.getOrElse(() => undefined))
                : val;
            const emptyEncoded = () => {
                switch (empty.kind) {
                    case "null":
                        return null;
                    case "undefined":
                        return undefined;
                    case "effect-option":
                        return Option.none();
                }
            };
            const pair = {
                toValue: (val) => v.option(isEmpty(val) ? undefined : innerPair.toValue(unwrap(val))),
                fromValue: (sv) => {
                    const ov = sv;
                    if (ov.value === undefined)
                        return emptyEncoded();
                    return wrap(innerPair.fromValue(ov.value));
                },
                wire: innerPair.wire && {
                    write: (val, writer) => writer.add({
                        tag: "option-value",
                        val: isEmpty(val) ? undefined : innerPair.wire.write(unwrap(val), writer),
                    }),
                    read: (nodes, index) => {
                        const child = wireNode(nodes, index, "option-value").val;
                        return child === undefined ? emptyEncoded() : wrap(innerPair.wire.read(nodes, child));
                    },
                },
            };
            return { type: t.option(innerType), pair };
        };
        /** Build a type + pair for a single AST node (no graph mutation). */
        const buildNode = (a) => Effect.gen(function* () {
            const native = annotationOf(a, witSchemaNodeAnnotationKey);
            if (native !== undefined) {
                switch (native.tag) {
                    case "text":
                        return {
                            type: schemaType(native),
                            pair: {
                                toValue: v.text,
                                fromValue: (x) => x.text,
                                wire: {
                                    write: (text, writer) => writer.add({ tag: "text-value", val: { text, language: undefined } }),
                                    read: (nodes, index) => wireNode(nodes, index, "text-value").val.text,
                                },
                            },
                        };
                    case "binary":
                        return {
                            type: schemaType(native),
                            pair: {
                                toValue: v.binary,
                                fromValue: (x) => x.bytes,
                                wire: {
                                    write: (bytes, writer) => writer.add({ tag: "binary-value", val: { bytes, mimeType: undefined } }),
                                    read: (nodes, index) => wireNode(nodes, index, "binary-value").val.bytes,
                                },
                            },
                        };
                    case "path":
                        return { type: t.path(native.spec), pair: primPair(v.path) };
                    case "url":
                        return { type: t.url(native.restrictions), pair: primPair(v.url) };
                    case "uuid":
                        return { type: t.uuid(), pair: primPair(v.uuid) };
                    case "datetime":
                        return { type: t.datetime(), pair: primPair(v.datetime) };
                    case "duration":
                        return {
                            type: t.duration(),
                            pair: {
                                toValue: v.duration,
                                fromValue: (x) => x.nanoseconds,
                                wire: {
                                    write: (nanoseconds, writer) => writer.add({ tag: "duration-value", val: { nanoseconds } }),
                                    read: (nodes, index) => wireNode(nodes, index, "duration-value").val.nanoseconds,
                                },
                            },
                        };
                    case "quantity":
                        return { type: t.quantity(native.spec), pair: primPair(v.quantity) };
                    case "fixed-list": {
                        if (a._tag !== "Arrays" || a.rest.length !== 1) {
                            return yield* unsupported("FixedList annotation must be attached to Schema.Array");
                        }
                        const item = yield* child(a.rest[0]);
                        return {
                            type: t.fixedList(item.type, native.length),
                            pair: {
                                toValue: (xs) => v.fixedList(xs.map(item.pair.toValue)),
                                wire: sequenceWire("fixed-list-value", [item.pair], native.length),
                                fromValue: (x) => x.elements.map(item.pair.fromValue),
                            },
                        };
                    }
                    case "map": {
                        if (a._tag !== "Declaration" || a.typeParameters.length < 2) {
                            return yield* unsupported("Map annotation must be attached to Schema.ReadonlyMap");
                        }
                        const key = yield* child(a.typeParameters[0]);
                        const value = yield* child(a.typeParameters[1]);
                        return {
                            type: t.map(key.type, value.type),
                            pair: {
                                wire: mapWire(key.pair, value.pair, true),
                                toValue: (map) => v.map([...map].map(([k, val]) => ({
                                    key: key.pair.toValue(k),
                                    value: value.pair.toValue(val),
                                }))),
                                fromValue: (x) => new Map(x.entries.map((entry) => [
                                    key.pair.fromValue(entry.key),
                                    value.pair.fromValue(entry.value),
                                ])),
                            },
                        };
                    }
                    case "flags": {
                        const duplicate = native.names.find((name, i) => native.names.indexOf(name) !== i);
                        if (native.names.length === 0)
                            return yield* unsupported("flags has no names");
                        if (duplicate !== undefined) {
                            return yield* unsupported(`duplicate flag name '${duplicate}'`);
                        }
                        return {
                            type: t.flags([...native.names]),
                            pair: {
                                wire: {
                                    write: (flags, writer) => writer.add({ tag: "flags-value", val: [...flags] }),
                                    read: (nodes, index) => {
                                        const flags = wireNode(nodes, index, "flags-value").val;
                                        if (flags.length !== native.names.length)
                                            throw new Error(`value: expected ${native.names.length} flags`);
                                        return [...flags];
                                    },
                                },
                                toValue: (flags) => v.flags([...flags]),
                                fromValue: (value) => [
                                    ...value.flags,
                                ],
                            },
                        };
                    }
                    case "discriminated-union": {
                        const branches = [];
                        for (const branch of native.branches) {
                            const compiled = yield* child(SchemaAST.toEncoded(branch.schema.ast));
                            branches.push({
                                model: {
                                    tag: branch.tag,
                                    body: compiled.type,
                                    discriminator: branch.discriminator,
                                    metadata: compiled.type.metadata,
                                },
                                pair: compiled.pair,
                            });
                        }
                        const type = t.union(branches.map((branch) => branch.model));
                        const errors = validateSchemaGraph({ defs, root: type });
                        if (errors.length > 0) {
                            return yield* unsupported(errors.map((error) => error.message).join("; "));
                        }
                        return {
                            type,
                            pair: {
                                toValue: (value) => {
                                    const matches = branches.filter((branch) => discriminatorMatches(branch.model.discriminator, value));
                                    if (matches.length !== 1) {
                                        throw new Error(`discriminated union value matched ${matches.length} branches; expected exactly one`);
                                    }
                                    const branch = matches[0];
                                    return v.union(branch.model.tag, branch.pair.toValue(value));
                                },
                                fromValue: (value) => {
                                    const union = value;
                                    const branch = branches.find((entry) => entry.model.tag === union.unionTag);
                                    if (branch === undefined) {
                                        throw new Error(`unknown discriminated union tag '${union.unionTag}'`);
                                    }
                                    const decoded = branch.pair.fromValue(union.body);
                                    if (!discriminatorMatches(branch.model.discriminator, decoded)) {
                                        throw new Error(`discriminated union branch '${branch.model.tag}' does not match its discriminator`);
                                    }
                                    return decoded;
                                },
                            },
                        };
                    }
                    case "secret": {
                        if (a._tag !== "Declaration" || a.typeParameters[0] === undefined) {
                            return yield* unsupported("Secret requires an inner schema");
                        }
                        const inner = yield* child(a.typeParameters[0]);
                        return {
                            type: t.secret(inner.type, { category: native.category }),
                            pair: {
                                toValue: (handle) => v.secret(handle),
                                fromValue: (x) => {
                                    const handle = x.handle;
                                    if (peekGuestSecretHandle(SECRET_INTERNAL, handle) === undefined) {
                                        throw new Error("secret handle was already consumed");
                                    }
                                    return handle;
                                },
                            },
                        };
                    }
                    case "quota-token":
                        return {
                            type: t.quotaToken({ resourceName: native.resourceName }),
                            pair: quotaTokenNode(native.resourceName).pair,
                        };
                    case "permission-card":
                        return {
                            type: t.permissionCard(native.spec),
                            pair: {
                                toValue: (handle) => v.permissionCard(handle),
                                // A received card takes the kind that the schema declares.
                                fromValue: (x) => {
                                    const handle = x.handle;
                                    if (peekGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, handle) === undefined) {
                                        throw new Error("permission-card handle was already consumed");
                                    }
                                    return assignGuestPermissionCardHandleKind(PERMISSION_CARD_INTERNAL, handle, native.spec.polymorphic ? "template" : "concrete");
                                },
                            },
                        };
                    case "stream": {
                        if (a._tag !== "Declaration" || a.typeParameters[0] === undefined) {
                            return yield* unsupported("AgentStream requires an element schema");
                        }
                        const itemAst = a.typeParameters[0];
                        const item = yield* child(itemAst);
                        const itemSchema = native.elementSchema ?? Schema.make(itemAst);
                        let itemCodec;
                        const codec = () => (itemCodec ??= Effect.runSync(toWitCodec(itemSchema))).codec;
                        return {
                            type: t.stream(item.type),
                            pair: {
                                toValue: (stream) => v.stream(agentStreamToHandle(stream, codec(), requireConversionContext())),
                                fromValue: (x) => agentStreamFromHandle(x.handle, codec(), requireConversionContext()),
                            },
                        };
                    }
                }
            }
            switch (a._tag) {
                case "String": {
                    // `Char` annotates Schema.Char with witType: "char".
                    const witHint = annotationOf(a, witTypeAnnotationKey);
                    if (witHint === "char") {
                        return { type: t.char(), pair: primPair(v.char) };
                    }
                    return { type: t.string(), pair: primPair(v.string) };
                }
                case "Boolean":
                    return { type: t.bool(), pair: primPair(v.bool) };
                case "Literal": {
                    // Preserve the received payload so Effect Schema validates the literal.
                    const lit = a.literal;
                    if (typeof lit === "string") {
                        return {
                            type: t.string(),
                            pair: primPair(v.string),
                        };
                    }
                    if (typeof lit === "boolean") {
                        return { type: t.bool(), pair: primPair(v.bool) };
                    }
                    if (typeof lit === "number") {
                        return { type: t.f64(), pair: primPair(v.f64) };
                    }
                    if (typeof lit === "bigint") {
                        return { type: t.s64(), pair: primPair(v.s64) };
                    }
                    return yield* unsupported(`unsupported literal type: ${typeof lit}`);
                }
                case "Number": {
                    const kind = numericKindOf(a) ?? "f64";
                    return numericNode(kind, a);
                }
                case "BigInt": {
                    const kind = numericKindOf(a) ?? "s64";
                    return numericNode(kind, a);
                }
                case "Enum": {
                    if (a.enums.length === 0)
                        return yield* unsupported("empty enum");
                    const allStrings = a.enums.every(([, value]) => typeof value === "string");
                    const cases = allStrings
                        ? a.enums.map(([, value]) => value)
                        : a.enums.map(([name]) => name);
                    const values = a.enums.map(([, value]) => value);
                    return {
                        type: t.enum(cases),
                        pair: {
                            toValue: (value) => v.enum(values.indexOf(value)),
                            fromValue: (value) => values[value.caseIndex],
                            wire: {
                                write: (value, writer) => writer.add({ tag: "enum-value", val: values.indexOf(value) }),
                                read: (nodes, index) => {
                                    const caseIndex = wireNode(nodes, index, "enum-value").val;
                                    if (caseIndex >= values.length)
                                        throw new Error("value: enum case index out of range");
                                    return values[caseIndex];
                                },
                            },
                        },
                    };
                }
                case "Objects": {
                    if (a.indexSignatures.length > 0) {
                        return yield* unsupported("index signatures cannot be represented in the schema model");
                    }
                    return yield* recordNode(a.propertySignatures);
                }
                case "Arrays": {
                    if (a.rest.length === 0 && a.elements.length > 0) {
                        const elemTypes = [];
                        const elemPairs = [];
                        for (const el of a.elements) {
                            const { type, pair } = yield* child(el);
                            elemTypes.push(type);
                            elemPairs.push(pair);
                        }
                        return {
                            type: t.tuple(elemTypes),
                            pair: {
                                wire: sequenceWire("tuple-value", elemPairs, elemPairs.length),
                                toValue: (arr) => v.tuple(elemPairs.map((p, i) => p.toValue(arr[i]))),
                                fromValue: (sv) => {
                                    const tv = sv;
                                    return elemPairs.map((p, i) => p.fromValue(tv.elements[i]));
                                },
                            },
                        };
                    }
                    if (a.elements.length === 0 && a.rest.length === 1) {
                        const { type, pair } = yield* child(a.rest[0]);
                        return {
                            type: t.list(type),
                            pair: {
                                toValue: (arr) => v.list(arr.map((x) => pair.toValue(x))),
                                fromValue: (sv) => {
                                    const lv = sv;
                                    return lv.elements.map((c) => pair.fromValue(c));
                                },
                                wire: pair.wire && {
                                    write: (arr, writer) => writer.add({
                                        tag: "list-value",
                                        val: arr.map((x) => pair.wire.write(x, writer)),
                                    }),
                                    read: (nodes, index) => wireNode(nodes, index, "list-value").val.map((i) => pair.wire.read(nodes, i)),
                                },
                            },
                        };
                    }
                    return yield* unsupported("mixed tuple/rest arrays are not supported");
                }
                case "Union":
                    return yield* unionNode(a);
                case "Declaration": {
                    // The opaque `quota-token` capability is a declared schema marked
                    // with `witQuotaTokenAnnotationKey`; emit the dedicated capability
                    // node rather than treating it as an unknown declared type.
                    if (isQuotaTokenAST(a)) {
                        return quotaTokenNode(quotaTokenResourceNameOf(a));
                    }
                    // A `Principal` carried as data (annotated via `PrincipalSchema`):
                    // emit the `principal` variant rather than an unknown declared type.
                    if (isPrincipalAST(a)) {
                        return principalNode();
                    }
                    // Typed-array hints (Uint8ArraySchema, …) take precedence — emit a
                    // dedicated `list<primN>` shape rather than an unknown declared type.
                    const tak = typedArrayKindOf(a);
                    if (tak !== undefined) {
                        const elem = typedArrayElement[tak];
                        const Ctor = typedArrayCtor[tak];
                        const item = primPair(elem.toV, elem.make().body.tag, elem.coerce);
                        return {
                            type: t.list(elem.make()),
                            pair: {
                                toValue: (arr) => {
                                    const items = [];
                                    for (const x of arr)
                                        items.push(elem.toV(elem.coerce(x)));
                                    return v.list(items);
                                },
                                fromValue: (sv) => {
                                    const lv = sv;
                                    const raw = lv.elements.map((c) => c.value);
                                    return new Ctor(raw);
                                },
                                wire: {
                                    write: (arr, writer) => writer.add({
                                        tag: "list-value",
                                        val: Array.from(arr, (value) => item.wire.write(value, writer)),
                                    }),
                                    read: (nodes, index) => new Ctor(wireNode(nodes, index, "list-value").val.map((i) => item.wire.read(nodes, i))),
                                },
                            },
                        };
                    }
                    return yield* declarationNode(a);
                }
                default:
                    return yield* unsupported(`unsupported AST node: ${a._tag}`);
            }
        });
        const nodeFor = (raw, alreadyEncoded = false) => Effect.gen(function* () {
            if (raw._tag === "Suspend") {
                const thunk = raw.thunk;
                return yield* nodeFor(thunk(), true);
            }
            const key = raw;
            const existing = entries.get(key);
            if (existing !== undefined) {
                if (existing.active)
                    existing.recursive = true;
                if (existing.recursive || existing.active) {
                    return {
                        type: t.ref(existing.id),
                        pair: {
                            toValue: (value) => existing.pair.toValue(value),
                            fromValue: (value) => existing.pair.fromValue(value),
                        },
                    };
                }
                return yield* buildNode(alreadyEncoded ? raw : SchemaAST.toEncoded(raw));
            }
            const entry = {
                id: `effect-schema-${nextId++}`,
                active: true,
                recursive: false,
            };
            entries.set(key, entry);
            const a = alreadyEncoded ? raw : SchemaAST.toEncoded(raw);
            entries.set(a, entry);
            const built = yield* buildNode(a);
            entry.active = false;
            entry.pair = built.pair;
            const type = { ...built.type, metadata: metadataOf(a) };
            if (entry.recursive) {
                const title = a.annotations?.title;
                defs.set(entry.id, {
                    ...(typeof title === "string" ? { name: title } : {}),
                    body: type,
                });
                return { type: t.ref(entry.id), pair: built.pair };
            }
            return { type, pair: built.pair };
        });
        const unionNode = (a) => Effect.gen(function* () {
            // String-literal enum: every member is a string Literal.
            if (a.types.length > 0 &&
                a.types.every((m) => SchemaAST.isLiteral(m) && typeof m.literal === "string")) {
                const literals = a.types.map((m) => m.literal);
                return {
                    type: t.enum(literals),
                    pair: {
                        toValue: (s) => v.enum(literals.indexOf(s)),
                        fromValue: (sv) => literals[sv.caseIndex],
                    },
                };
            }
            // NullOr / UndefinedOr: a union with exactly one Null OR Undefined/Void
            // member and exactly one "real" member maps to `option<inner>`.
            // NullishOr (Null + Undefined + T) is intentionally NOT collapsed — both
            // empty kinds can't round-trip through a single option, so it falls
            // through to the generic variant path.
            const emptyMembers = a.types.filter(isNullOrUndefinedAST);
            const realMembers = a.types.filter((m) => !isNullOrUndefinedAST(m));
            if (emptyMembers.length === 1 && realMembers.length === 1) {
                const empty = emptyMembers[0]._tag === "Null" ? "null" : "undefined";
                const { type, pair } = yield* child(realMembers[0]);
                return optionWrap(type, pair, { kind: empty });
            }
            if (a.types.length === 0) {
                return yield* unsupported("empty union");
            }
            // Tagged variant: every member is an Objects with a *required*
            // string-literal _tag. Falls through to generic variant otherwise.
            if (a.types.every(SchemaAST.isObjects)) {
                const tagged = a.types.every((m) => {
                    const tagPs = m.propertySignatures.find((ps) => ps.name === "_tag");
                    return (!!tagPs &&
                        !SchemaAST.isOptional(tagPs.type) &&
                        SchemaAST.isLiteral(tagPs.type) &&
                        typeof tagPs.type.literal === "string");
                });
                if (tagged) {
                    return yield* taggedVariantNode(a);
                }
            }
            // Generic variant: arbitrary union members.
            return yield* genericVariantNode(a);
        });
        /**
         * Build a tagged variant where every member is an Objects with a
         * string-literal `_tag` discriminator.
         */
        const taggedVariantNode = (a) => Effect.gen(function* () {
            const cases = [];
            const seenTags = new Set();
            for (const m of a.types) {
                const tagPs = m.propertySignatures.find((ps) => ps.name === "_tag");
                const tag = tagPs.type.literal;
                if (seenTags.has(tag)) {
                    return yield* unsupported(`duplicate variant tag '${tag}' in tagged union`);
                }
                seenTags.add(tag);
                const rest = m.propertySignatures.filter((ps) => ps.name !== "_tag");
                if (rest.length === 0) {
                    cases.push({ tag, payloadType: undefined, payloadPair: undefined });
                }
                else {
                    const { type, pair } = yield* recordNode(rest);
                    cases.push({ tag, payloadType: type, payloadPair: pair });
                }
            }
            const tagToIdx = new Map(cases.map((c, i) => [c.tag, i]));
            const variantCases = cases.map((c) => variantCase(c.tag, c.payloadType, c.payloadType?.metadata));
            return {
                type: t.variant(variantCases),
                pair: {
                    toValue: (obj) => {
                        const i = tagToIdx.get(obj._tag);
                        if (i === undefined)
                            throw new Error(`unknown variant tag: ${obj._tag}`);
                        const c = cases[i];
                        if (c.payloadPair === undefined)
                            return v.variant(i, undefined);
                        const { _tag, ...rest } = obj;
                        return v.variant(i, c.payloadPair.toValue(rest));
                    },
                    fromValue: (sv) => {
                        const vv = sv;
                        const c = cases[vv.caseIndex];
                        if (c.payloadPair === undefined || vv.payload === undefined) {
                            return { _tag: c.tag };
                        }
                        return { _tag: c.tag, ...c.payloadPair.fromValue(vv.payload) };
                    },
                },
            };
        });
        /**
         * Build a generic variant for an arbitrary `Schema.Union(...)`. Each member
         * becomes a variant case (auto-named `caseN` or annotated via
         * {@link withVariantCaseName}). Encode picks the first member whose
         * `EncodedShape.matches(encoded)` returns true, in declaration order; decode
         * uses the case index.
         */
        const genericVariantNode = (a) => Effect.gen(function* () {
            // Conflict detection on encoded shapes.
            const shapes = a.types.map(encodedShapeOf);
            const seen = new Set();
            let plainObjectCount = 0;
            let plainArrayCount = 0;
            let taggedObjectCount = 0;
            const literalTypeKinds = new Set();
            const primitiveKinds = new Set();
            for (const s of shapes) {
                if (s.tag === "object")
                    plainObjectCount++;
                if (s.tag === "array")
                    plainArrayCount++;
                if (s.tag === "object-with-tag")
                    taggedObjectCount++;
                if (s.tag === "literal")
                    literalTypeKinds.add(typeof s.literal);
                if (s.tag === "string" ||
                    s.tag === "number" ||
                    s.tag === "boolean" ||
                    s.tag === "bigint") {
                    primitiveKinds.add(s.tag);
                }
                const key = s.tag === "literal"
                    ? `literal:${typeof s.literal}:${String(s.literal)}`
                    : s.tag === "object-with-tag"
                        ? `object-with-tag:${s.tagLiteral}`
                        : s.tag;
                if (s.tag !== "object" && s.tag !== "array" && s.tag !== "unknown" && seen.has(key)) {
                    return yield* unsupported(`ambiguous union: two members share encoded shape '${key}'`);
                }
                seen.add(key);
            }
            if (plainObjectCount > 1) {
                return yield* unsupported("ambiguous union: multiple object members without distinct `_tag` discriminators");
            }
            if (plainObjectCount > 0 && taggedObjectCount > 0) {
                return yield* unsupported("ambiguous union: cannot mix plain object members with `_tag`-discriminated object members");
            }
            if (plainArrayCount > 1) {
                return yield* unsupported("ambiguous union: multiple array members cannot be distinguished");
            }
            const primitiveOfLiteralKind = {
                string: "string",
                number: "number",
                boolean: "boolean",
                bigint: "bigint",
            };
            for (const litKind of literalTypeKinds) {
                const conflict = primitiveOfLiteralKind[litKind];
                if (conflict !== undefined && primitiveKinds.has(conflict)) {
                    return yield* unsupported(`ambiguous union: primitive '${conflict}' member overlaps a literal of the same type`);
                }
            }
            if (shapes.some((s) => s.tag === "unknown") && shapes.length > 1) {
                return yield* unsupported("ambiguous union: contains a member whose encoded shape cannot be classified for variant dispatch");
            }
            const usedNames = new Set();
            const cases = [];
            for (let i = 0; i < a.types.length; i++) {
                const m = a.types[i];
                const annotated = variantCaseNameOf(m);
                const name = annotated ?? `case${i}`;
                if (usedNames.has(name)) {
                    return yield* unsupported(`duplicate variant case name '${name}' in Schema.Union`);
                }
                usedNames.add(name);
                const shape = encodedShapeOf(m);
                if (m._tag === "Null" || m._tag === "Undefined" || m._tag === "Void") {
                    cases.push({
                        name,
                        type: undefined,
                        pair: undefined,
                        matches: shape.matches,
                        emptyEncoded: m._tag === "Null" ? null : undefined,
                    });
                }
                else {
                    const { type, pair } = yield* child(m);
                    cases.push({ name, type, pair, matches: shape.matches, emptyEncoded: undefined });
                }
            }
            const variantCases = cases.map((c) => variantCase(c.name, c.type, c.type?.metadata));
            return {
                type: t.variant(variantCases),
                pair: {
                    toValue: (val) => {
                        for (let i = 0; i < cases.length; i++) {
                            const c = cases[i];
                            if (c.matches(val)) {
                                if (c.pair === undefined)
                                    return v.variant(i, undefined);
                                return v.variant(i, c.pair.toValue(val));
                            }
                        }
                        throw new Error(`Schema.Union: no member matched value of type ${typeof val}`);
                    },
                    fromValue: (sv) => {
                        const vv = sv;
                        const c = cases[vv.caseIndex];
                        if (c.pair === undefined || vv.payload === undefined)
                            return c.emptyEncoded;
                        return c.pair.fromValue(vv.payload);
                    },
                },
            };
        });
        /**
         * Recognised `Schema.declareConstructor`-based types mapped to their schema
         * shape: `Schema.Option`, `Schema.Result`, `Schema.ReadonlyMap`,
         * `Schema.HashMap`.
         */
        const declarationNode = (a) => Effect.gen(function* () {
            const tc = declarationConstructorTag(a);
            switch (tc) {
                case "effect/Option": {
                    const inner = a.typeParameters[0];
                    if (inner === undefined) {
                        return yield* unsupported("Schema.Option without inner type parameter");
                    }
                    const { type, pair } = yield* child(inner);
                    return optionWrap(type, pair, { kind: "effect-option" });
                }
                case "effect/Result": {
                    const okAst = a.typeParameters[0];
                    const errAst = a.typeParameters[1];
                    if (okAst === undefined || errAst === undefined) {
                        return yield* unsupported("Schema.Result without both type parameters");
                    }
                    const { type: okType, pair: okPair } = yield* child(okAst);
                    const { type: errType, pair: errPair } = yield* child(errAst);
                    const pair = {
                        wire: okPair.wire && errPair.wire
                            ? {
                                write: (r, writer) => writer.add({
                                    tag: "result-value",
                                    val: Result.isSuccess(r)
                                        ? { tag: "ok-value", val: okPair.wire.write(r.success, writer) }
                                        : { tag: "err-value", val: errPair.wire.write(r.failure, writer) },
                                }),
                                read: (nodes, index) => {
                                    const result = wireNode(nodes, index, "result-value").val;
                                    if (result.val === undefined)
                                        throw new Error("value: result payload is required");
                                    return result.tag === "ok-value"
                                        ? Result.succeed(okPair.wire.read(nodes, result.val))
                                        : Result.fail(errPair.wire.read(nodes, result.val));
                                },
                            }
                            : undefined,
                        toValue: (val) => {
                            const r = val;
                            return Result.isSuccess(r)
                                ? v.ok(okPair.toValue(r.success))
                                : v.err(errPair.toValue(r.failure));
                        },
                        fromValue: (sv) => {
                            const rv = sv.result;
                            if (rv.tag === "ok") {
                                return Result.succeed(rv.value === undefined ? undefined : okPair.fromValue(rv.value));
                            }
                            return Result.fail(rv.value === undefined ? undefined : errPair.fromValue(rv.value));
                        },
                    };
                    return { type: t.result(okType, errType), pair };
                }
                case "ReadonlyMap":
                case "effect/HashMap": {
                    const kAst = a.typeParameters[0];
                    const vAst = a.typeParameters[1];
                    if (kAst === undefined || vAst === undefined) {
                        return yield* unsupported("Schema.ReadonlyMap/HashMap without both type parameters");
                    }
                    const { type: kType, pair: kPair } = yield* child(kAst);
                    const { type: vType, pair: vPair } = yield* child(vAst);
                    const isHashMap = tc === "effect/HashMap";
                    // Represented as `list<tuple<k, v>>` so arbitrary (non-primitive)
                    // key types are allowed, matching the previous model's behaviour.
                    const pair = {
                        wire: mapWire(kPair, vPair, false, isHashMap),
                        toValue: (val) => {
                            const entries = isHashMap
                                ? HashMap.toEntries(val)
                                : val.entries();
                            const items = [];
                            for (const [k, val2] of entries) {
                                items.push(v.tuple([kPair.toValue(k), vPair.toValue(val2)]));
                            }
                            return v.list(items);
                        },
                        fromValue: (sv) => {
                            const lv = sv;
                            const entries = lv.elements.map((it) => {
                                const tv = it;
                                return [kPair.fromValue(tv.elements[0]), vPair.fromValue(tv.elements[1])];
                            });
                            return isHashMap ? HashMap.fromIterable(entries) : new Map(entries);
                        },
                    };
                    return { type: t.list(t.tuple([kType, vType])), pair };
                }
                default:
                    return yield* unsupported(`unsupported declaration: ${tc ?? "unknown"}`);
            }
        });
        // Walk on the *encoded* AST so user-defined `decodeTo` chains (Schema.Option,
        // Schema.Result, custom record↔class bridges, …) surface their wire shape.
        const root = yield* nodeFor(ast);
        return { ...root, defs };
    });
};
/**
 * Build a `WitCodec<S>` for a single Effect Schema. Composes:
 *
 *     userSchema (Type ↔ Encoded)
 *       ↕  per-AST value transforms (encoded ↔ SchemaValue)
 *
 * into one `Codec<S["Type"], SchemaValue>`. Refinements / transformations
 * inside the user's schema run as part of the outer codec, so we get validation
 * and good error messages for free. The flat `schema-value-tree` wire carrier is
 * produced from `SchemaValue` at the dispatch boundary.
 *
 * @since 1.6.0
 * @category codecs
 */
const toWitCodec = (schema) => Effect.gen(function* () {
    const EncodedCarrier = Schema.declare((_u) => true);
    const SchemaValueCarrier = Schema.declare((_u) => true);
    // Void/undefined returns map to WIT `output-schema.unit`; the graph + codec
    // are placeholders the agent layer ignores when `isUnit` is set.
    const encodedAst = SchemaAST.toEncoded(schema.ast);
    const isUnit = isVoidLikeAST(encodedAst);
    const walked = isUnit ? undefined : yield* walk(schema.ast);
    const pair = walked?.pair ?? {
        toValue: () => v.record([]),
        fromValue: () => undefined,
    };
    const root = walked?.type ?? t.record([]);
    const graph = { defs: walked?.defs ?? new Map(), root };
    const svToEncoded = SchemaValueCarrier.pipe(Schema.decodeTo(EncodedCarrier, {
        decode: SchemaGetter.transformEffect((sv, options) => {
            const converted = Effect.try({
                try: () => {
                    assertValueShape(graph, root, sv);
                    return sv;
                },
                catch: (error) => new SchemaIssue.InvalidValue({
                    message: error instanceof Error ? error.message : String(error),
                }, sv, options),
            });
            return Effect.flatMap(converted, (checked) => Effect.flatMap(Effect.context(), (context) => Effect.try({
                try: () => withConversionContext(context, () => pair.fromValue(checked)),
                catch: (error) => new SchemaIssue.InvalidValue({
                    message: error instanceof Error ? error.message : String(error),
                }, sv, options),
            })));
        }),
        encode: SchemaGetter.transformEffect((enc, options) => Effect.flatMap(Effect.context(), (context) => Effect.try({
            try: () => withConversionContext(context, () => pair.toValue(enc)),
            catch: (error) => new SchemaIssue.InvalidValue({
                message: error instanceof Error ? error.message : String(error),
            }, enc, options),
        }))),
    }));
    const codec = svToEncoded.pipe(Schema.decodeTo(schema));
    if (pair.wire) {
        const direct = pair.wire;
        const carrier = Schema.declare((_) => true);
        const wireCodec = carrier.pipe(Schema.decodeTo(EncodedCarrier, {
            decode: SchemaGetter.transformEffect((tree, options) => Effect.try({
                try: () => {
                    return direct.read(tree.valueNodes, tree.root);
                },
                catch: (error) => new SchemaIssue.InvalidValue({ message: error instanceof Error ? error.message : String(error) }, tree, options),
            })),
            encode: SchemaGetter.transformEffect((value, options) => Effect.try({
                try: () => {
                    const writer = new WireWriter();
                    const root = direct.write(value, writer);
                    return { valueNodes: writer.nodes, root };
                },
                catch: (error) => new SchemaIssue.InvalidValue({ message: error instanceof Error ? error.message : String(error) }, value, options),
            })),
        }), Schema.decodeTo(schema));
        wireCodecs.set(codec, wireCodec);
    }
    return {
        schema,
        graph,
        isUnit,
        codec,
    };
});
const wireSchemaError = (error) => new Schema.SchemaError(new SchemaIssue.InvalidValue({
    message: error instanceof Error ? error.message : String(error),
}));
/** Decode a complete wire value while retaining ownership until validation succeeds.
 * @since 1.6.0 @category codecs
 */
const decodeFromWire = (codec, value) => makeWireDecoder(codec)(value);
/** Prepare a wire decoder once, with a fresh ownership transaction for every execution.
 * @since 1.6.0 @category codecs
 */
const makeWireDecoder = (codec) => {
    const decode = Schema.decodeEffect(codec);
    const direct = wireCodecs.get(codec);
    const decodeDirect = direct && Schema.decodeEffect(direct);
    return (value) => withCapabilityTransaction((transaction) => Effect.flatMap(Effect.try({
        try: () => !!decodeDirect &&
            !value.valueNodes.some((node) => node?.tag === "secret-value" ||
                node?.tag === "quota-token-handle" ||
                node?.tag === "permission-card-handle" ||
                node?.tag === "stream-value"),
        catch: wireSchemaError,
    }), (useDirect) => useDirect
        ? Effect.flatMap(Effect.try({
            try: () => {
                preflightWitValueTree(value.valueNodes, value.root);
                return value;
            },
            catch: wireSchemaError,
        }), decodeDirect)
        : Effect.flatMap(Effect.try({
            try: () => schemaValueFromWit(value, transaction),
            catch: wireSchemaError,
        }), decode)));
};
/** Prepare a wire encoder, retaining the resource transaction path for unsupported shapes.
 * @since 1.6.0 @category codecs
 */
const makeWireEncoder = (codec, asynchronous = true, typedWireErrors = false) => {
    const sync = (run) => typedWireErrors ? Effect.try({ try: run, catch: wireSchemaError }) : Effect.sync(run);
    const async = (run) => typedWireErrors ? Effect.tryPromise({ try: run, catch: wireSchemaError }) : Effect.promise(run);
    const direct = wireCodecs.get(codec);
    if (direct) {
        const encode = Schema.encodeEffect(direct);
        const validate = (tree) => {
            preflightWitValueTree(tree.valueNodes, tree.root);
            return tree;
        };
        return (value) => Effect.flatMap(encode(value), (tree) => asynchronous
            ? async((signal) => restoreEncodedCapabilitiesOnAbort(signal, Promise.resolve().then(() => validate(tree))))
            : sync(() => validate(tree)));
    }
    const encode = Schema.encodeEffect(codec);
    return (value) => Effect.flatMap(encode(value), (encoded) => asynchronous
        ? async((signal) => restoreEncodedCapabilitiesOnAbort(signal, schemaValueToWitAsync(encoded, signal)))
        : sync(() => schemaValueToWit(encoded)));
};
/** Prepare structural wire operations for a named record from its existing child codecs.
 * @since 1.6.0 @category codecs
 */
const prepareWireRecordCodec = (codec, entries) => {
    const children = entries.map((entry) => wireCodecs.get(entry.codec));
    if (children.some((child) => child === undefined))
        return;
    const carrier = Schema.declare((_) => true);
    const records = Schema.declare((_) => true);
    const childRecord = Schema.Struct(Object.fromEntries(entries.map((entry, i) => [entry.name, children[i]])));
    const transform = (run) => SchemaGetter.transformEffect((value, options) => Effect.try({
        try: () => run(value),
        catch: (error) => new SchemaIssue.InvalidValue({ message: error instanceof Error ? error.message : String(error) }, value, options),
    }));
    const direct = carrier.pipe(Schema.decodeTo(records, {
        decode: transform((tree) => {
            const fields = wireNode(tree.valueNodes, tree.root, "record-value").val;
            if (fields.length !== entries.length)
                throw new Error(`expected record with ${entries.length} fields`);
            return Object.fromEntries(entries.map((entry, i) => [
                entry.name,
                { valueNodes: tree.valueNodes, root: fields[i] },
            ]));
        }),
        encode: transform((record) => {
            const nodes = [];
            const roots = [];
            for (const entry of entries) {
                const tree = record[entry.name];
                const offset = nodes.length;
                roots.push(tree.root + offset);
                for (const node of tree.valueNodes) {
                    if (offset === 0) {
                        nodes.push(node);
                        continue;
                    }
                    switch (node.tag) {
                        case "record-value":
                        case "tuple-value":
                        case "list-value":
                        case "fixed-list-value":
                            nodes.push({ tag: node.tag, val: node.val.map((i) => i + offset) });
                            break;
                        case "map-value":
                            nodes.push({
                                tag: node.tag,
                                val: node.val.map((entry) => ({
                                    key: entry.key + offset,
                                    value: entry.value + offset,
                                })),
                            });
                            break;
                        case "option-value":
                            nodes.push({
                                tag: node.tag,
                                val: node.val === undefined ? undefined : node.val + offset,
                            });
                            break;
                        case "result-value":
                            nodes.push({
                                tag: node.tag,
                                val: {
                                    ...node.val,
                                    val: node.val.val === undefined ? undefined : node.val.val + offset,
                                },
                            });
                            break;
                        case "variant-value":
                            nodes.push({
                                tag: node.tag,
                                val: {
                                    ...node.val,
                                    payload: node.val.payload === undefined ? undefined : node.val.payload + offset,
                                },
                            });
                            break;
                        case "union-value":
                            nodes.push({ tag: node.tag, val: { ...node.val, body: node.val.body + offset } });
                            break;
                        default:
                            nodes.push(node);
                    }
                }
            }
            const root = nodes.length;
            nodes.push({ tag: "record-value", val: roots });
            return { valueNodes: nodes, root };
        }),
    }), Schema.decodeTo(childRecord));
    wireCodecs.set(codec, direct);
};

class ConfigClient extends Context.Service()("effect-golem/host/Config") {
}
const ConfigLive = Layer.succeed(ConfigClient, ConfigClient.of({
    getConfigValue: (key, expected) => AgentHost.getConfigValue([...key], expected),
}));

class SecretsClient extends Context.Service()("effect-golem/host/Secrets") {
}
const SecretsLive = Layer.succeed(SecretsClient, SecretsClient.of({
    reveal: RevealHost.reveal,
    id: SecretsHost.id,
    metadata: SecretsHost.metadata,
}));

class ConfigError {
    path;
    reason;
    _tag = "ConfigError";
    constructor(path, reason) {
        this.path = path;
        this.reason = reason;
    }
}
const encodeOverrides = (compiled, overrides) => Effect.gen(function* () {
    const out = [];
    const visit = (value, path) => Effect.gen(function* () {
        const key = path.join("/");
        const leaf = compiled.leavesByPath.get(key);
        if (leaf === undefined) {
            if (compiled.branches.has(key) && typeof value === "object" && value !== null) {
                for (const [name, child] of Object.entries(value))
                    yield* visit(child, [...path, name]);
                return;
            }
            return yield* Effect.fail(new ConfigError(path, {
                _tag: "Unsupported",
                reason: `unknown override path: ${path.join(".")}`,
            }));
        }
        if (leaf.source === "secret") {
            return yield* Effect.fail(new ConfigError(path, {
                _tag: "Unsupported",
                reason: `cannot override secret config path: ${path.join(".")}`,
            }));
        }
        out.push({
            path: [...path],
            value: {
                graph: leaf.codec.schemaGraph,
                value: yield* leaf.codec.encode(value),
            },
        });
    });
    for (const [name, value] of Object.entries(overrides))
        yield* visit(value, [name]);
    return out;
});

/**
 * Canonical schema and lossless conversions for
 * `wasi:clocks/types@0.3.0`.Datetime.
 *
 * JavaScript `Date`, Effect `DateTime`, and numeric epoch timestamps have
 * millisecond precision. Converting a WIT datetime to one of those forms
 * therefore rejects sub-millisecond values rather than truncating them.
 *
 * @since 1.5.1
 */
const U64_MAX = 2n ** 64n - 1n;
const NANOS_PER_SECOND = 1_000_000_000;
const NANOS_PER_MILLISECOND = 1_000_000;
const MILLIS_PER_SECOND = 1000n;
const MAX_DATE_EPOCH_MILLISECONDS = 8_640_000_000_000_000;
const Seconds = Uint64.check(Schema.isBetweenBigInt({ minimum: 0n, maximum: U64_MAX })).annotate({
    [witTypeAnnotationKey]: "u64",
});
const Nanoseconds = Uint32.check(Schema.isInt(), Schema.isBetween({ minimum: 0, maximum: NANOS_PER_SECOND - 1 })).annotate({ [witTypeAnnotationKey]: "u32" });
/**
 * Schema for `wasi:clocks/types@0.3.0`.Datetime.
 *
 * The seconds field is an unsigned 64-bit integer and nanoseconds must be in
 * the range 0 through 999,999,999, matching the WIT clock contract.
 *
 * @since 1.5.1
 * @category codecs
 */
const Datetime = Schema.Struct({
    seconds: Seconds,
    nanoseconds: Nanoseconds,
});
/**
 * A datetime could not be represented without losing precision or exceeding
 * the range supported by the target representation.
 *
 * @since 1.5.1
 * @category errors
 */
class DatetimeConversionError {
    message;
    _tag = "DatetimeConversionError";
    constructor(message) {
        this.message = message;
    }
}
const conversionError = (message) => new DatetimeConversionError(message);
const validate = (value) => Schema.decodeUnknownEffect(Datetime)(value).pipe(Effect.mapError((cause) => conversionError(`Invalid WIT datetime: ${String(cause)}`)));
/**
 * Convert non-negative epoch milliseconds to a WIT datetime.
 *
 * The input must be an integer within JavaScript's valid `Date` range. Negative
 * timestamps are rejected because the WIT seconds field is unsigned.
 *
 * @since 1.5.1
 * @category conversions
 */
const fromEpochMilliseconds = (epochMilliseconds) => {
    if (!Number.isSafeInteger(epochMilliseconds)) {
        return Effect.fail(conversionError("Epoch milliseconds must be a finite safe integer"));
    }
    if (epochMilliseconds < 0) {
        return Effect.fail(conversionError("WIT datetime cannot represent an instant before the Unix epoch"));
    }
    if (epochMilliseconds > MAX_DATE_EPOCH_MILLISECONDS) {
        return Effect.fail(conversionError("Epoch milliseconds exceed the JavaScript Date range"));
    }
    const seconds = Math.floor(epochMilliseconds / Number(MILLIS_PER_SECOND));
    const milliseconds = epochMilliseconds % Number(MILLIS_PER_SECOND);
    return Effect.succeed({
        seconds: BigInt(seconds),
        nanoseconds: milliseconds * NANOS_PER_MILLISECOND,
    });
};
/**
 * Convert a valid JavaScript `Date` to a WIT datetime.
 *
 * @since 1.5.1
 * @category conversions
 */
const fromDate = (date) => {
    const epochMilliseconds = date.getTime();
    if (Number.isNaN(epochMilliseconds)) {
        return Effect.fail(conversionError("Cannot convert an invalid JavaScript Date"));
    }
    return fromEpochMilliseconds(epochMilliseconds);
};
/**
 * Convert an Effect `DateTime` to a WIT datetime. Zoned values are converted by
 * their absolute instant; their display timezone does not change the result.
 *
 * @since 1.5.1
 * @category conversions
 */
const fromDateTime = (dateTime) => fromEpochMilliseconds(dateTime.epochMilliseconds);
/**
 * Normalize a WIT datetime, JavaScript `Date`, Effect `DateTime`, or numeric
 * epoch-millisecond value to a validated WIT datetime.
 *
 * @since 1.5.1
 * @category conversions
 */
const fromInput = (input) => {
    if (typeof input === "number")
        return fromEpochMilliseconds(input);
    if (input instanceof Date)
        return fromDate(input);
    if (DateTime.isDateTime(input))
        return fromDateTime(input);
    return validate(input);
};

/**
 * Host service for the execution-mode subset of
 * `golem:api/host@1.5.0`:
 *
 * - `oplogCommit` (replication barrier)
 * - `generateIdempotencyKey` (persisted UUID generator used by RPC)
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class DurabilityModeClient extends Context.Service()("effect-golem/host/DurabilityMode") {
}
const DurabilityModeLive = Layer.succeed(DurabilityModeClient, DurabilityModeClient.of({
    oplogCommit: (replicas) => ApiHost.oplogCommit(replicas),
    generateIdempotencyKey: () => ApiHost.generateIdempotencyKey(),
}));

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
// Schema-graph construction helpers.
//
// `SchemaBuilder` registers named (nominal) types into a graph using a
// reserve/commit protocol so recursive and mutually-recursive types close
// without infinite recursion: a type reserves its id before walking its body,
// so a back-reference encountered during the walk short-circuits to a `ref`.
//
// `mergeAgentGraphs` combines the per-root graphs of an agent's constructor,
// methods, and config into a single graph, deduplicating shared definitions by
// stable `type-id` and rejecting divergent definitions of the same id.
const generatedEffectSchemaId = /^effect-schema-(?:\d+-)*\d+$/;
const remapTypeRefs = (type, ids) => {
    const remap = (child) => remapTypeRefs(child, ids);
    const body = type.body;
    switch (body.tag) {
        case "ref":
            return { ...type, body: { ...body, id: ids.get(body.id) ?? body.id } };
        case "record":
            return {
                ...type,
                body: {
                    ...body,
                    fields: body.fields.map((field) => ({ ...field, body: remap(field.body) })),
                },
            };
        case "variant":
            return {
                ...type,
                body: {
                    ...body,
                    cases: body.cases.map((variant) => ({
                        ...variant,
                        payload: variant.payload === undefined ? undefined : remap(variant.payload),
                    })),
                },
            };
        case "tuple":
            return { ...type, body: { ...body, elements: body.elements.map(remap) } };
        case "list":
        case "fixed-list":
        case "option":
            return { ...type, body: { ...body, element: remap(body.element) } };
        case "map":
            return { ...type, body: { ...body, key: remap(body.key), value: remap(body.value) } };
        case "result":
            return {
                ...type,
                body: {
                    ...body,
                    ok: body.ok === undefined ? undefined : remap(body.ok),
                    err: body.err === undefined ? undefined : remap(body.err),
                },
            };
        case "union":
            return {
                ...type,
                body: {
                    ...body,
                    branches: body.branches.map((branch) => ({ ...branch, body: remap(branch.body) })),
                },
            };
        case "secret":
            return { ...type, body: { ...body, inner: remap(body.inner) } };
        case "future":
        case "stream":
            return {
                ...type,
                body: { ...body, element: body.element === undefined ? undefined : remap(body.element) },
            };
        default:
            return type;
    }
};
/** Merge definitions from several graphs, rejecting conflicting same-id bodies. */
function mergeGraphDefs(graphs) {
    const merged = new Map();
    for (const g of graphs) {
        for (const [id, def] of g.defs) {
            const existing = merged.get(id);
            if (existing) {
                if (!deepEqual(existing.body, def.body)) {
                    throw new SchemaConflictError(id);
                }
            }
            else {
                merged.set(id, def);
            }
        }
    }
    return merged;
}
/**
 * Compose independently compiled schema graphs. Compiler-generated Effect
 * definition ids are scoped by root position and every reference is rewritten;
 * explicit ids retain their conflict-detecting, nominal semantics.
 */
function composeSchemaGraphs(graphs, scope = "composed") {
    let scopeHash = 2166136261;
    for (let i = 0; i < scope.length; i++) {
        scopeHash ^= scope.charCodeAt(i);
        scopeHash = Math.imul(scopeHash, 16777619);
    }
    const scopeId = scopeHash >>> 0;
    const remapped = graphs.map((graph, graphIndex) => {
        const ids = new Map();
        for (const id of graph.defs.keys()) {
            if (generatedEffectSchemaId.test(id)) {
                ids.set(id, `effect-schema-${scopeId}-${graphIndex}-${id.slice(14)}`);
            }
        }
        return {
            root: remapTypeRefs(graph.root, ids),
            defs: new Map([...graph.defs].map(([id, def]) => [
                ids.get(id) ?? id,
                { ...def, body: remapTypeRefs(def.body, ids) },
            ])),
        };
    });
    return { defs: mergeGraphDefs(remapped), roots: remapped.map((graph) => graph.root) };
}

/**
 * @since 1.5.0
 */
/**
 * A `SchemaGetter.transformEffect` that catches any error the pure transform
 * throws and surfaces it as a `SchemaIssue.InvalidValue` (so it lands on the
 * schema-error channel rather than escaping as an uncaught defect).
 *
 * @since 1.5.0
 * @category codecs
 */
const tryGetter = (f) => SchemaGetter.transformEffect((input, options) => {
    try {
        return Effect.succeed(f(input));
    }
    catch (e) {
        return Effect.fail(new SchemaIssue.InvalidValue({
            message: e instanceof Error ? e.message : String(e),
        }, input, options));
    }
});
/**
 * Type-guard for `ElementSpec` carriers.
 *
 * @since 1.5.0
 * @category guards
 */
const isElementSpec = (x) => typeof x === "object" &&
    x !== null &&
    x._effectGolem === "ElementSpec";
/**
 * Type-guard for `OptionalElementSpec` carriers.
 *
 * @since 1.6.0
 * @category guards
 */
const isOptionalElementSpec = (x) => typeof x === "object" &&
    x !== null &&
    x._effectGolem === "OptionalElementSpec";

/**
 * @since 1.5.0
 */
/**
 * Type-guard for `Multimodal` carriers.
 *
 * @since 1.5.0
 * @category guards
 */
const isMultimodal = (x) => typeof x === "object" &&
    x !== null &&
    x._effectGolem === "Multimodal";

/**
 * @since 1.5.0
 */
/** Schema for principals as ordinary values and auto-injected agent inputs. @since 1.6.0 @category schemas */
Schema.declare((value) => {
    if (typeof value !== "object" || value === null || !("tag" in value))
        return false;
    const tag = value.tag;
    return tag === "oidc" || tag === "agent" || tag === "golem-user" || tag === "anonymous";
}).pipe(Schema.annotate({ [witPrincipalAnnotationKey]: true }));
/**
 * Effect service exposing the active {@link Principal}.
 *
 * The framework provides this service automatically at the dispatch
 * boundary:
 *
 * - inside an agent's `impl` (constructor) effect it resolves to the
 *   immediate caller that `golem:agent/invocation-context.caller`
 *   reports during `agent-guest.guest.initialize`;
 * - inside a method handler it resolves to the immediate caller of
 *   that specific `agent-guest.guest.invoke` call (which may differ
 *   from the initialize-time principal, e.g. when other callers reach
 *   a durable agent instance).
 *
 * The dispatcher always provides this service before running user code,
 * so depending on it from inside an agent never leaks into the public
 * `R` slot of method/constructor signatures. In tests that bypass the
 * dispatcher you must provide it explicitly with
 * `Effect.provideService(Principal, …)`.
 *
 * **Example**
 *
 * ```ts
 * impl: () =>
 *   Effect.gen(function* () {
 *     const owner = yield* Principal
 *     return {
 *       whoCalled: () =>
 *         Effect.gen(function* () {
 *           const caller = yield* Principal
 *           return caller.tag
 *         }),
 *     }
 *   })
 * ```
 *
 * @since 1.5.0
 * @category host services
 */
class Principal extends Context.Service()("effect-golem/Principal") {
}

/**
 * @since 1.5.0
 */
/**
 * Make a plain object pipeable by setting its prototype to
 * `Pipeable.Prototype`. Returns a fresh object with the same own
 * enumerable properties as `obj`; `pipe` is a prototype-delegated
 * (i.e. non-own) method, so it does NOT show up in:
 *
 * - `Object.keys(...)`
 * - object spread (`{ ...x }`)
 * - `JSON.stringify(...)`
 *
 * It does still appear in `for...in` traversals because `Pipeable.Prototype.pipe`
 * is enumerable on the prototype — that mirrors how every other
 * pipeable Effect value (`Effect`, `Schema`, `Layer`, ...) behaves.
 *
 * Used on factory return values for declarative DSL records
 * (`EndpointDef`, `MountDef`, `MethodSpec`) so users can compose
 * cross-cutting facets with the canonical Effect `.pipe(...)` style.
 * The literal-object form (`{ ..., http: [...] }`) keeps working — the
 * pipeable form is purely additive.
 *
 * **Example**
 *
 * ```ts
 * Http.get("/x").pipe(Http.withAuth(true), Http.withCors("https://x.com"))
 * method({ params: ..., success: ... }).pipe(
 *   Method.withHttp(Http.get("/x")),
 *   Method.withDescription("..."),
 * )
 * ```
 *
 * @since 1.5.0
 * @category utils
 */
const withPipe = (obj) => Object.assign(Object.create(Pipeable.Prototype), obj);

/**
 * @since 1.6.0
 */
/** @since 1.6.0 @category constructors */
const method = (spec) => withPipe({ error: Schema.Void, ...spec });
const isPrincipal = (schema) => SchemaAST.resolveAt(witPrincipalAnnotationKey)(schema.ast) === true;
const paramCodec = (param) => isMultimodal(param)
    ? param.compile()
    : isElementSpec(param)
        ? Effect.succeed(param.witCodec)
        : toWitCodec(param);
/** Compile a named parameter record into one transactional record codec. @since 1.6.0 @category codecs */
const compileParamBindings = (context, input) => Effect.gen(function* () {
    const entries = [];
    for (const [name, param] of Object.entries(input)) {
        entries.push({
            name,
            codec: (yield* paramCodec(param)),
            injected: !isElementSpec(param) && !isMultimodal(param) && isPrincipal(param),
        });
    }
    if (entries.some((e) => isMultimodal(input[e.name])) && entries.length !== 1) {
        return yield* Effect.fail({
            _tag: "UnsupportedSchemaError",
            reason: `${context}: multimodal parameters must be the sole parameter (found ${entries.length})`,
        });
    }
    const composed = composeSchemaGraphs(entries.map((entry) => entry.codec.graph), context);
    const graph = {
        defs: composed.defs,
        root: t.record(entries.map((entry, index) => field(entry.name, composed.roots[index]))),
    };
    const encoder = new GraphEncoder(graph.defs);
    const roots = composed.roots.map((root) => encoder.encodeType(root));
    const schemaGraph = encoder.encodeGraphRoot(graph.root);
    const inputSchema = {
        tag: "parameters",
        val: entries.map((e, i) => ({
            name: e.name,
            source: e.injected ? { tag: "auto-injected", val: "principal" } : { tag: "user-supplied" },
            schema: roots[i],
            metadata: emptyMetadata(),
        })),
    };
    const Carrier = Schema.declare((_) => true);
    const EncodedRecord = Schema.Struct(Object.fromEntries(entries.map((e) => [e.name, e.codec.codec])));
    const valueToRecord = Carrier.pipe(Schema.decodeTo(Schema.declare((_) => true), {
        decode: tryGetter((sv) => {
            if (sv.tag !== "record" || sv.fields.length !== entries.length) {
                throw new Error(`${context}: expected record with ${entries.length} fields`);
            }
            return Object.fromEntries(entries.map((e, i) => [e.name, sv.fields[i]]));
        }),
        encode: tryGetter((record) => v.record(entries.map((e) => record[e.name]))),
    }));
    const codec = valueToRecord.pipe(Schema.decodeTo(EncodedRecord));
    prepareWireRecordCodec(codec, entries.map((entry) => ({ name: entry.name, codec: entry.codec.codec })));
    return {
        graph,
        schemaGraph,
        inputSchema,
        codec,
        encode: makeWireEncoder(codec, false),
        encodeAsync: makeWireEncoder(codec),
        decode: makeWireDecoder(codec),
    };
});
/** Compile only caller-supplied fields; principal fields are supplied by the host. @since 1.6.0 @category operations */
const compileCallerParamBindings = (context, input) => compileParamBindings(context, Object.fromEntries(Object.entries(input).filter(([, param]) => isElementSpec(param) || isMultimodal(param) || !isPrincipal(param))));
const isVoidSchema = (schema) => schema.ast._tag === "Void";
/**
 * `result<ok, err>` codec for an unstructured success element and a typed
 * error schema. The element is not an Effect Schema, so the result is built
 * from the two compiled codecs instead of `toWitCodec(Schema.Result(...))`.
 * Element graphs carry no nominal defs, so the error graph's defs are complete.
 */
const unstructuredResultCodec = (ok, err) => {
    const Carrier = Schema.declare((_) => true);
    const EncodedResult = Schema.declare((_) => true);
    const schema = Schema.Result(ok.codec, err.codec);
    const codec = Carrier.pipe(Schema.decodeTo(EncodedResult, {
        decode: tryGetter((sv) => {
            if (sv.tag !== "result")
                throw new Error(`Expected result value, got ${sv.tag}`);
            const value = sv.result.value;
            if (value === undefined)
                throw new Error(`Expected a ${sv.result.tag} payload`);
            return sv.result.tag === "ok" ? Result.succeed(value) : Result.fail(value);
        }),
        encode: tryGetter((result) => Result.isSuccess(result) ? v.ok(result.success) : v.err(result.failure)),
    }), Schema.decodeTo(schema));
    return {
        schema,
        graph: { defs: err.graph.defs, root: t.result(ok.graph.root, err.graph.root) },
        isUnit: false,
        codec: codec,
    };
};
const readOnlyConfig = (option, usesPrincipal) => {
    if (!option)
        return undefined;
    const value = option === true ? {} : option;
    const cache = value.cache;
    return {
        cachePolicy: cache === "no-cache"
            ? { tag: "no-cache" }
            : typeof cache === "object"
                ? { tag: "ttl", val: cache.ttlNanos }
                : { tag: "until-write" },
        usesPrincipal,
    };
};
/** Compile a complete method schema and its codecs. @since 1.6.0 @category codecs */
const compileMethodSpec = (name, spec) => Effect.gen(function* () {
    const inputCodec = yield* compileParamBindings(name, spec.input);
    const callerCodec = yield* compileCallerParamBindings(name, spec.input);
    const usesPrincipal = Object.values(spec.input).some((param) => !isElementSpec(param) && !isMultimodal(param) && isPrincipal(param));
    const errorWrapped = !isVoidSchema(spec.error);
    const success = spec.success;
    const unstructured = isElementSpec(success)
        ? success.witCodec
        : isOptionalElementSpec(success)
            ? success.witCodec
            : undefined;
    const successVoid = unstructured === undefined && isVoidSchema(success);
    const outputCodec = unstructured !== undefined
        ? errorWrapped
            ? unstructuredResultCodec(unstructured, yield* toWitCodec(spec.error))
            : unstructured
        : errorWrapped
            ? yield* toWitCodec(Schema.Result(successVoid ? Schema.Struct({}) : success, spec.error))
            : successVoid
                ? undefined
                : yield* toWitCodec(success);
    const composed = composeSchemaGraphs([inputCodec.graph, ...(outputCodec ? [outputCodec.graph] : [])], name);
    const graph = { defs: composed.defs, root: composed.roots[0] };
    const inputRoot = composed.roots[0];
    const outputType = outputCodec ? composed.roots[1] : undefined;
    const encoder = new GraphEncoder(graph.defs);
    const inputRoots = Object.values(spec.input).map((_, i) => {
        const root = inputRoot.body.fields[i].body;
        return encoder.encodeType(root);
    });
    const outputRoot = outputType ? encoder.encodeType(outputType) : undefined;
    const schemaGraph = encoder.finish();
    const inputSchema = {
        ...inputCodec.inputSchema,
        val: inputCodec.inputSchema.val.map((f, i) => ({ ...f, schema: inputRoots[i] })),
    };
    const encodeOutput = outputCodec && makeWireEncoder(outputCodec.codec);
    return {
        name,
        spec,
        graph,
        schemaGraph,
        inputCodec: callerCodec,
        outputCodec: outputCodec && outputType
            ? { ...outputCodec, graph: { defs: graph.defs, root: outputType } }
            : undefined,
        encodeOutput: encodeOutput === undefined ? undefined : encodeOutput,
        inputSchema,
        outputSchema: outputRoot === undefined ? { tag: "unit" } : { tag: "single", val: outputRoot },
        errorWrapped,
        successVoid,
        readOnly: readOnlyConfig(spec.readOnly, usesPrincipal),
    };
});
/** Invoke with concrete wire operations supplied by a generated contract. */
const invokeWireValue = (inputCodec, encodeOutput, options, handler, input) => Effect.gen(function* () {
    const decoded = yield* inputCodec.decode(input);
    const injected = options.inputSchema?.val.flatMap((field) => field.source.tag === "auto-injected" ? [{ name: field.name, kind: field.source.val }] : []) ?? [];
    const principal = injected.length > 0 ? yield* Principal : undefined;
    const fullInput = {
        ...decoded,
        ...Object.fromEntries(injected.map(({ name, kind }) => {
            switch (kind) {
                case "principal":
                    return [name, principal];
                default:
                    throw new Error(`Unsupported auto-injected input kind: ${kind}`);
            }
        })),
    };
    if (options.errorWrapped) {
        const result = yield* handler(fullInput).pipe(Effect.match({
            onFailure: (error) => Result.fail(error),
            onSuccess: (success) => Result.succeed(options.successVoid ? {} : success),
        }));
        return yield* encodeOutput(result);
    }
    const success = yield* handler(fullInput);
    if (encodeOutput === undefined)
        return undefined;
    return yield* encodeOutput(success);
});

/** Typed, schema-native RPC clients for agent definitions. @since 1.6.0 */
const responseError = (reason) => ({ _tag: "RemoteResponseError", reason });
class ClientBindingError {
    reason;
    _tag = "ClientBindingError";
    constructor(reason) {
        this.reason = reason;
    }
}
const bindIdentity = Symbol.for("effect-golem/client/bind-identity");
const decodeOutput = (mc, tree) => {
    if (mc.decodeOutput === undefined) {
        return tree === undefined
            ? Effect.succeed(undefined)
            : Effect.fail(responseError(`${mc.name}: expected unit output`));
    }
    if (tree === undefined)
        return Effect.fail(responseError(`${mc.name}: expected a result value`));
    return Effect.mapError(mc.decodeOutput(tree), (error) => responseError(`${mc.name}: failed to decode output: ${String(error)}`));
};
const finishOutput = (mc, value) => {
    if (!mc.errorWrapped)
        return Effect.succeed(value);
    const result = value;
    return Result.isSuccess(result)
        ? Effect.succeed(mc.successVoid ? undefined : result.success)
        : Effect.fail(result.failure);
};
const graphUsesStreams = (graph) => {
    const visited = new Set();
    const visit = (type) => {
        const body = type.body;
        switch (body.tag) {
            case "stream":
                return true;
            case "ref": {
                if (visited.has(body.id))
                    return false;
                visited.add(body.id);
                const definition = graph.defs.get(body.id);
                return definition !== undefined && visit(definition.body);
            }
            case "record":
                return body.fields.some((field) => visit(field.body));
            case "variant":
                return body.cases.some((item) => item.payload !== undefined && visit(item.payload));
            case "tuple":
                return body.elements.some(visit);
            case "list":
            case "fixed-list":
            case "option":
                return visit(body.element);
            case "map":
                return visit(body.key) || visit(body.value);
            case "result":
                return ((body.ok !== undefined && visit(body.ok)) || (body.err !== undefined && visit(body.err)));
            case "union":
                return body.branches.some((branch) => visit(branch.body));
            case "secret":
                return visit(body.inner);
            default:
                return false;
        }
    };
    return visit(graph.root);
};
const buildRemote = (rpc, compiled, ephemeral, defaultScopeCard) => {
    const remote = {};
    for (const [name, mc] of compiled.methods) {
        const streaming = mc.streaming;
        const rejectNonAwaitedStream = () => Effect.fail(responseError(`${name}: live streams require invoke-and-await`));
        const encode = (input) => Effect.mapError(mc.inputCodec.encodeAsync(input), (e) => responseError(`${name}: failed to encode input: ${String(e)}`));
        const awaited = (input, scopeCard) => awaitInvocation(rpc, name, encode(input), scopeCard).pipe(Effect.flatMap(({ metadata, result }) => decodeOutput(mc, result).pipe(Effect.flatMap((value) => finishOutput(mc, value)), Effect.map((value) => (ephemeral ? { metadata, value } : value)))));
        const trigger = (input) => {
            if (streaming)
                return rejectNonAwaitedStream();
            return encode(input).pipe(Effect.flatMap((tree) => Effect.try({
                try: () => invocationMetadataFromHost(rpc.invoke(name, tree)),
                catch: wrapHostThrow,
            })), Effect.map((metadata) => (ephemeral ? metadata : undefined)));
        };
        const schedule = (at, input) => {
            if (streaming)
                return rejectNonAwaitedStream();
            return fromInput(at).pipe(Effect.flatMap((time) => encode(input).pipe(Effect.map((tree) => [time, tree]))), Effect.flatMap(([time, tree]) => scheduleCancelableInvocation(rpc, time, name, tree)), Effect.map(({ metadata, cancel }) => {
                const scheduled = {
                    cancel: () => cancel,
                };
                return ephemeral ? { ...scheduled, metadata } : scheduled;
            }));
        };
        const call = (input) => awaited(input, defaultScopeCard);
        const withScopeCard = (card) => (input) => awaited(input, card);
        remote[name] = Object.assign(call, { trigger, schedule, withScopeCard });
    }
    return remote;
};
const bindingForDefinition = (definition) => {
    let cachedMethods;
    let cachedConstructor;
    const compile = Effect.gen(function* () {
        if (cachedMethods === undefined) {
            const methods = new Map();
            for (const [name, spec] of Object.entries(definition.methods)) {
                const mc = (yield* compileMethodSpec(name, spec));
                const output = mc.outputCodec;
                methods.set(name, {
                    name,
                    inputCodec: mc.inputCodec,
                    decodeOutput: output ? (tree) => decodeFromWire(output.codec, tree) : undefined,
                    streaming: graphUsesStreams(mc.inputCodec.graph) ||
                        (output !== undefined && graphUsesStreams(output.graph)),
                    errorWrapped: mc.errorWrapped,
                    successVoid: mc.successVoid,
                });
            }
            cachedMethods = methods;
        }
        if (definition.id !== undefined && cachedConstructor === undefined)
            cachedConstructor = yield* compileCallerParamBindings(`${definition.name ?? "agent"} constructor`, definition.id);
        return { methods: cachedMethods, constructorCodec: cachedConstructor };
    });
    const bindWithEntries = (identity, entries, options) => Effect.gen(function* () {
        if (definition.mode === "ephemeral") {
            return yield* Effect.fail(new ClientBindingError(`Cannot bind existing identity '${identity.encoded}' to ephemeral agent type '${identity.typeName}'; use getPhantom(...) or newPhantom(...)`));
        }
        if (definition.name !== undefined && identity.typeName !== definition.name) {
            return yield* Effect.fail(new ClientBindingError(`Full agent client '${definition.name}' cannot bind agent type '${identity.typeName}'`));
        }
        const compiled = yield* compile;
        if (compiled.constructorCodec !== undefined) {
            const constructor = compiled.constructorCodec.graph;
            if (!SchemaRef.fromImmutableGraph(constructor, constructor.root).validateValue(identity.constructorValue).success) {
                return yield* Effect.fail(new ClientBindingError(`Full agent client '${definition.name}' cannot bind identity '${identity.encoded}': constructor value does not conform to the client ID schema`));
            }
        }
        const rpc = yield* Effect.acquireRelease(connectIdentity(identity, entries, (cause) => new ClientBindingError(String(cause))), (connection) => Effect.sync(() => connection.drop()));
        return buildRemote(rpc, compiled, false, options?.scopeCard);
    });
    return {
        [bindIdentity]: (identity) => bindWithEntries(identity, []),
        bindWithEntries,
    };
};
/** Bind a parsed identity through a method-only, full, definition-owned, or reflected client. @since 1.6.0 @category constructors */
const bind$1 = (identity, binding) => binding[bindIdentity](identity);
/** @internal Build the binding protocol shared by `defineAgent` specs. */
const bindingFor = (definition) => bindingForDefinition(definition);
/** Build a client from the full agent-definition input: `name`, `mode`, `id`, `methods`, and optional `config`. */
const clientFor = (def) => {
    let cached;
    const compile = Effect.suspend(() => cached
        ? Effect.succeed(cached)
        : Effect.gen(function* () {
            const constructorCodec = yield* compileCallerParamBindings(`${def.name} constructor`, def.id);
            const methods = new Map();
            for (const [name, spec] of Object.entries(def.methods)) {
                const mc = yield* compileMethodSpec(name, spec);
                const output = mc.outputCodec;
                methods.set(name, {
                    name,
                    inputCodec: mc.inputCodec,
                    decodeOutput: output ? (tree) => decodeFromWire(output.codec, tree) : undefined,
                    streaming: graphUsesStreams(mc.inputCodec.graph) ||
                        (output !== undefined && graphUsesStreams(output.graph)),
                    errorWrapped: mc.errorWrapped,
                    successVoid: mc.successVoid,
                });
            }
            return (cached = { constructorCodec, methods });
        }));
    const config = (options) => Effect.gen(function* () {
        const values = [];
        if (options?.overrides !== undefined) {
            if (def.config === undefined)
                return yield* Effect.fail(new ConfigError([], {
                    _tag: "Unsupported",
                    reason: `agent '${def.name}' has no config; cannot apply overrides`,
                }));
            const encoded = yield* encodeOverrides(yield* def.config.__compile(), options.overrides).pipe(Effect.mapError((error) => error instanceof ConfigError
                ? error
                : responseError(`failed to encode config override: ${String(error)}`)));
            values.push(...encoded);
        }
        return values;
    });
    return clientForCompiled(def, compile, config);
};
const clientForCompiled = (def, compile, config = (options) => options?.overrides === undefined
    ? Effect.succeed([])
    : Effect.fail(new ConfigError([], {
        _tag: "Unsupported",
        reason: `agent '${def.name}' has no config; cannot apply overrides`,
    }))) => {
    const construct = (input, phantom, options) => Effect.gen(function* () {
        const codecs = yield* compile;
        const overrides = yield* config(options);
        const constructorTree = yield* Effect.mapError(codecs.constructorCodec.encodeAsync(input), (e) => responseError(`failed to encode constructor input: ${String(e)}`));
        const host = yield* RpcClient;
        const rpc = yield* Effect.acquireRelease(Effect.mapError(host.connect(def.name, constructorTree, phantom, overrides), (e) => wrapHostThrow(e.cause)), (connection) => Effect.sync(() => connection.drop()));
        return { rpc, codecs, scopeCard: options?.scopeCard };
    });
    const bindSelf = (options) => Effect.gen(function* () {
        const host = yield* RpcClient;
        const [id, rpc] = yield* Effect.acquireRelease(host.bindSelf().pipe(Effect.mapError((error) => wrapHostThrow(error.cause))), ([, connection]) => Effect.sync(() => connection.drop()));
        const identity = yield* parse(id);
        if (identity.typeName !== def.name)
            return yield* Effect.fail({
                _tag: "RpcCallError",
                cause: {
                    tag: "remote-agent-error",
                    val: {
                        tag: "invalid-type",
                        val: `self binding expects ${def.name}, identity is ${identity.typeName}`,
                    },
                },
            });
        const codecs = yield* compile;
        return buildRemote(rpc, codecs, false, options?.scopeCard);
    });
    const get = (input, options) => construct(input, undefined, options).pipe(Effect.map(({ rpc, codecs, scopeCard }) => buildRemote(rpc, codecs, false, scopeCard)));
    const getPhantom = (input, id, options) => Effect.try({
        try: () => parseUuid(id),
        catch: (e) => ({ _tag: "InvalidUuidError", value: id, reason: String(e) }),
    }).pipe(Effect.flatMap((uuid) => construct(input, uuid, options)), Effect.map(({ rpc, codecs, scopeCard }) => buildRemote(rpc, codecs, false, scopeCard)));
    const newPhantom = (input, options) => Effect.gen(function* () {
        if ((def.mode ?? "durable") === "ephemeral") {
            const { rpc, codecs, scopeCard } = yield* construct(input, undefined, options);
            return buildRemote(rpc, codecs, true, scopeCard);
        }
        const dm = yield* DurabilityModeClient;
        const uuid = yield* Effect.try({
            try: () => dm.generateIdempotencyKey(),
            catch: wrapHostThrow,
        });
        const { rpc, codecs, scopeCard } = yield* construct(input, uuid, options);
        return Object.assign(buildRemote(rpc, codecs, false, scopeCard), {
            phantomId: uuidToString(uuid),
        });
    });
    return ((def.mode ?? "durable") === "ephemeral"
        ? {
            getPhantom: (input, id, options) => Effect.try({
                try: () => parseUuid(id),
                catch: (e) => ({
                    _tag: "InvalidUuidError",
                    value: id,
                    reason: String(e),
                }),
            }).pipe(Effect.flatMap((uuid) => construct(input, uuid, options)), Effect.map(({ rpc, codecs, scopeCard }) => buildRemote(rpc, codecs, true, scopeCard))),
            newPhantom,
        }
        : { bindSelf, get, getPhantom, newPhantom });
};

/** The host rejected construction of an agent identity. @since 1.6.0 @category errors */
class AgentIdentityError {
    cause;
    _tag = "AgentIdentityError";
    constructor(cause) {
        this.cause = cause;
    }
}

const dynamicMethod = (rpc, name) => ({
    name,
    invoke: (input, options) => awaitInvocation(rpc, name, Effect.succeed(input), options?.scopeCard, false).pipe(Effect.map(({ metadata, result }) => ({ metadata, value: result }))),
    trigger: (input) => Effect.try({
        try: () => invocationMetadataFromHost(rpc.invoke(name, input)),
        catch: wrapHostThrow,
    }),
    schedule: (at, input) => scheduleCancelableInvocation(rpc, at, name, input),
});

/** Schema-value RPC for callers that only have an environment-scoped agent ID. @since 1.6.0 */
/**
 * Bind directly to an agent ID without a reflection lookup. Inputs and outputs
 * are schema-value trees, so the caller owns the independent method definitions.
 * @since 1.6.0
 * @category constructors
 */
const bind = (identity) => Effect.gen(function* () {
    const rpc = yield* Effect.acquireRelease(connectIdentity(identity, [], (cause) => new AgentIdentityError(cause)), (connection) => Effect.sync(() => connection.drop()));
    return Object.freeze({
        agentId: identity.encoded,
        method: (name) => dynamicMethod(rpc, name),
    });
});

/** Parsed, environment-scoped agent identities. @since 1.6.0 */
const copyValue = (value) => {
    if (value instanceof Uint8Array)
        return value.slice();
    if (Array.isArray(value))
        return value.map(copyValue);
    if (value !== null && typeof value === "object") {
        const prototype = Object.getPrototypeOf(value);
        if (prototype === Object.prototype || prototype === null)
            return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, copyValue(item)]));
    }
    // Native capability handles are opaque and cannot be cloned or frozen.
    return value;
};
const deepFreeze = (value, seen = new WeakSet()) => {
    if (value === null || typeof value !== "object" || seen.has(value))
        return value;
    if (value instanceof Uint8Array)
        return value;
    const prototype = Object.getPrototypeOf(value);
    if (!Array.isArray(value) && prototype !== Object.prototype && prototype !== null)
        return value;
    seen.add(value);
    for (const item of Object.values(value))
        deepFreeze(item, seen);
    return Object.freeze(value);
};
const create = (encoded, typeName, constructorValue, phantom) => {
    const immutableConstructor = copyValue(constructorValue);
    const identity = Object.freeze({
        encoded,
        typeName,
        get constructorValue() {
            return deepFreeze(copyValue(immutableConstructor));
        },
        client: (binding) => bind$1(identity, binding),
        dynamicClient: () => bind(identity),
        ...(phantom === undefined ? {} : { phantomId: uuidToString(phantom) }),
    });
    retainRawPhantomId(identity, phantom);
    return identity;
};
/** Strictly parse an encoded identity once and retain its immutable semantic parts. @since 1.6.0 @category constructors */
const parse = (encoded) => Effect.gen(function* () {
    const host = yield* AgentHostClient;
    return yield* Effect.try({
        try: () => {
            const [typeName, constructor, phantom] = decodeAgentId(host, encoded);
            return create(encoded, typeName, constructor.value, phantom);
        },
        catch: (cause) => new AgentIdentityError(cause),
    });
});
/** Construct an encoded identity from schema-native constructor data. @since 1.6.0 @category constructors */
const make = (options) => Effect.gen(function* () {
    const host = yield* AgentHostClient;
    return yield* Effect.try({
        try: () => {
            const phantom = options.phantomId === undefined ? undefined : parseUuid(options.phantomId);
            const encoded = encodeAgentId(host, options.typeName, options.constructorValue, phantom);
            return create(encoded, options.typeName, options.constructorValue, phantom);
        },
        catch: (cause) => new AgentIdentityError(cause),
    });
});

/**
 * Host service for `wasi:cli/environment@0.3.0`. Wraps the
 * `getEnvironment()` host call so SDK code can read the process
 * environment via Effect-typed DI rather than a direct ESM specifier
 * import.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class EnvironmentClient extends Context.Service()("effect-golem/host/Environment") {
}
const EnvironmentLive = Layer.succeed(EnvironmentClient, EnvironmentClient.of({
    getEnvironment: Effect.sync(() => WasiEnv.getEnvironment()),
}));

/**
 * Prefer a strict UTF-8 decoder, but follow the official Golem TypeScript SDK
 * by falling back when the runtime rejects the `fatal` option.
 *
 * wasm-rquickjs 0.3.6 throws from its JavaScript `TextDecoder` constructor even
 * though its native Rust decoder implements fatal decoding and the generated
 * guest enables the `encoding` feature. There is currently no feature flag
 * that bypasses that JavaScript guard. The fallback is intentionally lenient:
 * malformed UTF-8 is replaced with U+FFFD until GOL-324 is fixed upstream.
 *
 * @internal
 */
const strictTextDecoder = () => {
    try {
        return new TextDecoder("utf-8", { fatal: true });
    }
    catch {
        return new TextDecoder("utf-8");
    }
};

/**
 * Effect-idiomatic wrapper around `wasi:blobstore/blobstore` +
 * `wasi:blobstore/container` + `wasi:blobstore/types`.
 *
 * **Example**
 *
 * ```ts
 * import { Effect, Schema, Stream } from "effect"
 * import { Blobstore, defineAgent, method } from "@golemcloud/effect-golem"
 *
 * defineAgent({
 *   name: "Photos",
 *   constructorParams: { name: Schema.String },
 *   methods: {
 *     upload: method({ params: { key: Schema.String, body: Schema.Uint8Array }, success: Schema.Void }),
 *     download: method({ params: { key: Schema.String }, success: Schema.Uint8Array }),
 *     list: method({ params: {}, success: Schema.Array(Schema.String) }),
 *   },
 *   impl: ({ name }) =>
 *     Effect.gen(function* () {
 *       const photos = yield* Blobstore.getOrCreateContainer(name)
 *       return {
 *         upload: ({ key, body }) => photos.writeData(key, body),
 *         download: ({ key }) => photos.getData(key),
 *         list: () => Stream.runCollect(photos.listObjects).pipe(Effect.map((c) => Array.from(c))),
 *       }
 *     }),
 * })
 * ```
 *
 * @since 1.5.0
 */
// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------
const messageOf = (e) => {
    if (e !== null && typeof e === "object") {
        const obj = e;
        if (typeof obj.message === "string")
            return obj.message;
    }
    if (typeof e === "string")
        return e;
    return String(e);
};
/**
 * Raised when any host call into `wasi:blobstore/*` traps. Unlike
 * the keyvalue host the blobstore error type is a plain `string`
 * (per the WIT), so {@link trace} just carries that string verbatim.
 *
 * @since 1.5.0
 * @category errors
 */
class BlobstoreHostError {
    cause;
    operation;
    _tag = "BlobstoreHostError";
    message;
    trace;
    constructor(cause, operation) {
        this.cause = cause;
        this.operation = operation;
        const t = messageOf(cause);
        this.trace = t;
        this.message = `BlobstoreHostError(${operation}): ${t}`;
    }
}
// ---------------------------------------------------------------------------
// Schema codec — UTF-8 JSON over `Schema`
// ---------------------------------------------------------------------------
//
// We compose `Schema.fromJsonString(schema)` (the canonical
// JSON-string transformer in Effect v4) with a strict UTF-8 codec
// at the bytes ↔ string boundary. JSON syntax errors and schema
// validation failures uniformly surface as `Schema.SchemaError`;
// only malformed UTF-8 (which the schema layer does not see)
// surfaces as the SDK-internal {@link BlobstoreDecodeError}.
new TextEncoder();
strictTextDecoder();

/**
 * Host service for `wasi:blobstore/blobstore` +
 * `wasi:blobstore/container` + `wasi:blobstore/types` +
 * Preview 3 byte streams. Wraps the host's blobstore surface as
 * Effect-typed methods so SDK code can acquire / mutate object
 * containers via DI rather than reaching directly into the WIT
 * specifier imports.
 *
 * The Live `writeData` implementation owns the Preview 3 output stream;
 * consumers see a single `(name, bytes)` Effect.
 *
 * Resource handles are *not* exposed; the service surface is purely
 * by-value (records of `Effect`-returning methods). The public
 * {@link import("../Blobstore.js").Container} type wraps a
 * {@link HostContainer} returned from `createContainer` /
 * `getContainer` and adds the schema-typed view + JS `Date`
 * decoding + whole-object read recovery.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class BlobstoreClient extends Context.Service()("effect-golem/host/Blobstore") {
}
// ---------------------------------------------------------------------------
// Live impl — calls into wasi:blobstore/* + wasi:io/streams verbatim.
// ---------------------------------------------------------------------------
const consumeIncomingSync = (iv) => Effect.try({
    try: () => iv.incomingValueConsumeSync(),
    catch: (cause) => new BlobstoreHostError(cause, "incomingValueConsumeSync"),
});
const buildOutgoingValue = (bytes) => Effect.tryPromise({
    try: async () => {
        const ov = Types.OutgoingValue.newOutgoingValue();
        let complete;
        let fail;
        const consumed = new Promise((resolve, reject) => {
            complete = resolve;
            fail = reject;
        });
        const chunks = async function* () {
            let finished = false;
            try {
                yield* bytes;
                finished = true;
                complete();
            }
            finally {
                if (!finished)
                    fail(new Error("blob body consumer closed before all bytes were written"));
            }
        };
        ov.outgoingValueWriteBody(chunks());
        await consumed;
        return ov;
    },
    catch: (cause) => new BlobstoreHostError(cause, "outgoingValueWriteBody"),
});
const decodeContainerMetadata = (m) => ({
    name: m.name,
    createdAtMillis: m.createdAt,
});
const decodeObjectMetadata = (m) => ({
    name: m.name,
    container: m.container,
    createdAtMillis: m.createdAt,
    size: m.size,
});
const listObjectsStream = (handle) => Stream.unwrap(Effect.gen(function* () {
    const names = yield* Effect.try({
        try: () => handle.listObjects(),
        catch: (cause) => new BlobstoreHostError(cause, "container.listObjects"),
    });
    return Stream.fromAsyncIterable(names, (cause) => new BlobstoreHostError(cause, "container.listObjects"));
}));
const makeHostContainer = (name, handle) => {
    const info = Effect.gen(function* () {
        const m = yield* Effect.try({
            try: () => handle.info(),
            catch: (cause) => new BlobstoreHostError(cause, "container.info"),
        });
        return decodeContainerMetadata(m);
    });
    const clear = Effect.try({
        try: () => handle.clear(),
        catch: (cause) => new BlobstoreHostError(cause, "container.clear"),
    });
    const getData = (objectName, range) => Effect.gen(function* () {
        const iv = yield* Effect.try({
            try: () => handle.getData(objectName, range.start, range.end),
            catch: (cause) => new BlobstoreHostError(cause, "container.getData"),
        });
        return yield* consumeIncomingSync(iv);
    });
    const writeData = (objectName, data) => Effect.gen(function* () {
        const ov = yield* buildOutgoingValue(data);
        yield* Effect.try({
            try: () => handle.writeData(objectName, ov),
            catch: (cause) => new BlobstoreHostError(cause, "container.writeData"),
        });
    });
    const hasObject = (objectName) => Effect.try({
        try: () => handle.hasObject(objectName),
        catch: (cause) => new BlobstoreHostError(cause, "container.hasObject"),
    });
    const objectInfo = (objectName) => Effect.gen(function* () {
        const m = yield* Effect.try({
            try: () => handle.objectInfo(objectName),
            catch: (cause) => new BlobstoreHostError(cause, "container.objectInfo"),
        });
        return decodeObjectMetadata(m);
    });
    const deleteObject = (objectName) => Effect.try({
        try: () => handle.deleteObject(objectName),
        catch: (cause) => new BlobstoreHostError(cause, "container.deleteObject"),
    });
    const deleteObjects = (names) => Effect.try({
        try: () => handle.deleteObjects([...names]),
        catch: (cause) => new BlobstoreHostError(cause, "container.deleteObjects"),
    });
    return {
        name,
        info,
        clear,
        getData,
        writeData,
        hasObject,
        objectInfo,
        deleteObject,
        deleteObjects,
        get listObjects() {
            return listObjectsStream(handle);
        },
    };
};
const acquireHostContainer = (name, handle) => Effect.acquireRelease(Effect.succeed(makeHostContainer(name, handle)), () => Effect.void);
const liveCreateContainer = (name) => Effect.gen(function* () {
    const handle = yield* Effect.try({
        try: () => Blob.createContainer(name),
        catch: (cause) => new BlobstoreHostError(cause, "createContainer"),
    });
    return yield* acquireHostContainer(name, handle);
});
const liveGetContainer = (name) => Effect.gen(function* () {
    const handle = yield* Effect.try({
        try: () => Blob.getContainer(name),
        catch: (cause) => new BlobstoreHostError(cause, "getContainer"),
    });
    return yield* acquireHostContainer(name, handle);
});
const liveContainerExists = (name) => Effect.try({
    try: () => Blob.containerExists(name),
    catch: (cause) => new BlobstoreHostError(cause, "containerExists"),
});
const liveDeleteContainer = (name) => Effect.try({
    try: () => Blob.deleteContainer(name),
    catch: (cause) => new BlobstoreHostError(cause, "deleteContainer"),
});
const liveCopyObject = (src, dest) => Effect.try({
    try: () => Blob.copyObject(src, dest),
    catch: (cause) => new BlobstoreHostError(cause, "copyObject"),
});
const liveMoveObject = (src, dest) => Effect.try({
    try: () => Blob.moveObject(src, dest),
    catch: (cause) => new BlobstoreHostError(cause, "moveObject"),
});
const BlobstoreLive = Layer.succeed(BlobstoreClient, BlobstoreClient.of({
    createContainer: liveCreateContainer,
    getContainer: liveGetContainer,
    getOrCreateContainer: liveCreateContainer,
    containerExists: liveContainerExists,
    deleteContainer: liveDeleteContainer,
    copyObject: liveCopyObject,
    moveObject: liveMoveObject,
}));

/** Injectable host service for the runtime's custom invocation context. */
class CustomInvocationContextClient extends Context.Service()("effect-golem/host/CustomInvocationContext") {
}
const CustomInvocationContextLive = Layer.succeed(CustomInvocationContextClient, CustomInvocationContextClient.of({
    capture: () => Runtime.capture(),
    swap: (next) => Runtime.swap(next),
    run: (token, fn) => Runtime.run(token, fn),
    enterLive: (live) => Runtime.enterLive(live),
    close: (token) => Runtime.close(token),
}));

/** Injectable host service for `golem:durability/durability@1.6.0`. */
class DurabilityClient extends Context.Service()("effect-golem/host/Durability") {
}
const DurabilityLive = Layer.succeed(DurabilityClient, DurabilityClient.of({
    observeFunctionCall: (iface, function_) => DurabilityHost.observeFunctionCall(iface, function_),
    beginCustomDurableInvocationAsync: (functionName, request, functionType) => DurabilityHost.beginCustomDurableInvocationAsync(functionName, request, functionType),
    finishAsync: (invocation, response, forcedCommit) => DurabilityHost.LiveCustomDurableInvocation.finishAsync(invocation, response, forcedCommit),
    drop: (invocation) => invocation[Symbol.dispose](),
}));

/** Private, interruptible Effect seam for finite Durable Streams host attempts. */
class DurableStreamsClient extends Context.Service()("effect-golem/host/DurableStreams") {
}
const attempt = (run) => Effect.tryPromise({ try: run, catch: (cause) => cause }).pipe(Effect.catch((cause) => typeof cause === "object" && cause !== null && "kind" in cause && "message" in cause
    ? Effect.fail(cause)
    : Effect.die(cause)));
const DurableStreamsLive = Layer.succeed(DurableStreamsClient, DurableStreamsClient.of({
    makeReader: (options, auth) => Effect.map(scopedAttempt(() => new Host.DurableStreamReader(options, auth), (reader, request) => reader.read(request)), (read) => ({ read })),
    makeWriter: (options, auth) => Effect.map(scopedAttempt(() => new Host.DurableStreamWriter(options, auth), (writer, request) => writer.append(request)), (append) => ({ append })),
}));
// WIT async methods borrow their resource until their Promise settles. Effect interruption
// stops waiting, not the HTTP attempt: scope exit must defer drop while any borrow is live.
const scopedAttempt = (make, run) => Effect.acquireRelease(Effect.sync(() => {
    const resource = make();
    let active = 0;
    let released = false;
    const dispose = () => {
        if (released && active === 0)
            resource[Symbol.dispose]();
    };
    return {
        call: (request) => attempt(async () => {
            if (released)
                throw new Error("Durable Streams resource scope is closed");
            active++;
            try {
                return await run(resource, request);
            }
            finally {
                active--;
                dispose();
            }
        }),
        release: () => {
            released = true;
            dispose();
        },
    };
}), (state) => Effect.sync(state.release)).pipe(Effect.map((state) => state.call));

/**
 * Host service for `golem:rdbms/ignite2@1.5.0`. Wraps the synchronous
 * `DbConnection.open(address)` static entry point so SDK code can
 * acquire a connection via DI rather than reaching into the imported
 * namespace directly.
 *
 * Per the layer-mocks refactor plan §4c, the `DbConnection`,
 * `DbTransaction`, and `DbResultStream` resources themselves are NOT
 * services — only the constructor / entry point that produces them is.
 * Their methods (`query`, `execute`, `queryStream`, `beginTransaction`,
 * `commit`, `rollback`, …) are still synchronous and called from
 * `src/ignite.ts` inside `Effect.try` wrappers (see the surrounding
 * adapter for error classification).
 *
 * The user-facing `IgniteClient` adapter (`src/ignite.ts`) keeps its
 * byte-compatible public surface; the only change is that
 * `IgniteClient.make` / `IgniteClient.layer` now require
 * {@link IgniteHostClient} to be provided upstream. The agent
 * dispatcher's `provideUserRuntime` helper provides `HostLive` (which
 * contains {@link IgniteHostLive}), so user code inside `defineAgent`
 * impls never sees the requirement.
 *
 * Naming: `IgniteHostClient` (with the `HostClient` suffix) is
 * deliberately distinct from the user-facing `IgniteClient` adapter
 * that implements `effect/sql/SqlClient`.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class IgniteHostClient extends Context.Service()("effect-golem/host/Ignite") {
}
const IgniteHostLive = Layer.succeed(IgniteHostClient, IgniteHostClient.of({
    open: (address) => DbConnection.open(address),
}));

/**
 * Host service around `golem:agent/invocation-context@2.0.0`. The agent, tool, and middleware
 * export entry points read the immediate caller from it.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class InvocationContextClient extends Context.Service()("effect-golem/host/InvocationContext") {
}
const InvocationContextLive = Layer.succeed(InvocationContextClient, InvocationContextClient.of({
    caller: () => InvocationContext.caller(),
}));
/** The principal that made the running invocation. */
const immediateCaller = Effect.gen(function* () {
    const client = yield* InvocationContextClient;
    return yield* Effect.sync(() => client.caller().immediate);
});

/**
 * Effect-idiomatic wrapper around the `wasi:keyvalue@0.1.0` host
 * interface.
 *
 * Only the `eventual` (single-key CRUD) and `eventual-batch`
 * (multi-key CRUD) interfaces are exposed. The `atomic` interface
 * (`increment` / `compare-and-swap`) and the entire `cache` interface
 * are NOT wrapped because they are currently `unimplemented!` in the
 * Golem host (calling them traps the worker). When the host
 * implements them, this module will be extended to cover them.
 *
 * **Example**
 *
 * ```ts
 * import { Effect, Schema } from "effect"
 * import { defineAgent, KeyValue, method } from "@golemcloud/effect-golem"
 *
 * const User = Schema.Struct({ id: Schema.String, name: Schema.String })
 *
 * defineAgent({
 *   name: "Users",
 *   constructorParams: { name: Schema.String },
 *   methods: {
 *     put: method({ params: { id: Schema.String, name: Schema.String }, success: Schema.Void }),
 *     get: method({ params: { id: Schema.String }, success: Schema.Option(User) }),
 *   },
 *   impl: () =>
 *     Effect.gen(function* () {
 *       const bucket = yield* KeyValue.openBucket("users")
 *       const users = bucket.forSchema(User)
 *       return {
 *         put: ({ id, name }) => users.set(id, { id, name }),
 *         get: ({ id }) => users.get(id),
 *       }
 *     }),
 * })
 * ```
 *
 * @since 1.5.0
 */
// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------
const traceOf = (e) => {
    if (e !== null && typeof e === "object") {
        const obj = e;
        if (typeof obj.trace === "function") {
            try {
                return obj.trace();
            }
            catch {
                /* fall through */
            }
        }
        if (typeof obj.message === "string")
            return obj.message;
    }
    return String(e);
};
/**
 * Raised when any host call into `wasi:keyvalue@0.1.0` traps.
 *
 * The `trace` field is the verbatim driver-supplied string from the
 * host's `error.trace()` method (or, for non-Error host throws, the
 * stringified cause). Per the Golem source, this is opaque /
 * driver-specific (Redis / SQLite / Postgres / in-memory all produce
 * different formats) and should not be parsed.
 *
 * @since 1.5.0
 * @category errors
 */
class KeyValueHostError {
    cause;
    operation;
    _tag = "KeyValueHostError";
    message;
    trace;
    constructor(cause, operation) {
        this.cause = cause;
        this.operation = operation;
        const t = traceOf(cause);
        this.trace = t;
        this.message = `KeyValueHostError(${operation}): ${t}`;
    }
}
// ---------------------------------------------------------------------------
// Schema codec — UTF-8 JSON over `Schema`
// ---------------------------------------------------------------------------
//
// We compose `Schema.fromJsonString(schema)` (the canonical
// JSON-string transformer in Effect v4) with a strict UTF-8 codec
// at the bytes ↔ string boundary. JSON syntax errors and schema
// validation failures uniformly surface as `Schema.SchemaError`;
// only malformed UTF-8 (which the schema layer does not see)
// surfaces as the SDK-internal {@link KeyValueDecodeError}.
new TextEncoder();
strictTextDecoder();

/**
 * Host service for `wasi:keyvalue/types@0.1.0` +
 * `wasi:keyvalue/eventual@0.1.0` +
 * `wasi:keyvalue/eventual-batch@0.1.0`. Wraps the host's keyvalue
 * surface as Effect-typed methods so SDK code can acquire / mutate
 * buckets via DI rather than reaching directly into the WIT
 * specifier imports.
 *
 * The `IncomingValue` / `OutgoingValue` resource plumbing
 * (`incomingValueConsumeSync` / `outgoingValueWriteBodySync`) lives
 * inside the Live impl; consumers see a plain `Uint8Array`.
 *
 * Resource handles are *not* exposed; the service surface is purely
 * by-value (records of `Effect`-returning methods). The public
 * {@link import("../KeyValue.js").Bucket} type wraps a
 * {@link HostBucket} returned from `openBucket` and adds the
 * `BucketTypeId` stamp + the schema-typed `forSchema` view.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class KeyValueClient extends Context.Service()("effect-golem/host/KeyValue") {
}
// ---------------------------------------------------------------------------
// Live impl — calls into wasi:keyvalue/* verbatim.
// ---------------------------------------------------------------------------
const writeOutgoing = (bytes) => Effect.try({
    try: () => {
        const ov = KvTypes.OutgoingValue.newOutgoingValue();
        ov.outgoingValueWriteBodySync(bytes);
        return ov;
    },
    catch: (cause) => new KeyValueHostError(cause, "outgoingValueWriteBodySync"),
});
const consumeIncoming = (iv) => Effect.try({
    try: () => iv.incomingValueConsumeSync(),
    catch: (cause) => new KeyValueHostError(cause, "incomingValueConsumeSync"),
});
const makeHostBucket = (name, handle) => {
    const get = (key) => Effect.gen(function* () {
        const iv = yield* Effect.try({
            try: () => KvEventual.get(handle, key),
            catch: (cause) => new KeyValueHostError(cause, "eventual.get"),
        });
        if (iv === undefined || iv === null)
            return Option.none();
        const bytes = yield* consumeIncoming(iv);
        return Option.some(bytes);
    });
    const set = (key, value) => Effect.gen(function* () {
        const ov = yield* writeOutgoing(value);
        yield* Effect.try({
            try: () => KvEventual.set(handle, key, ov),
            catch: (cause) => new KeyValueHostError(cause, "eventual.set"),
        });
    });
    const del = (key) => Effect.try({
        try: () => KvEventual.delete_(handle, key),
        catch: (cause) => new KeyValueHostError(cause, "eventual.delete"),
    });
    const exists = (key) => Effect.try({
        try: () => KvEventual.exists(handle, key),
        catch: (cause) => new KeyValueHostError(cause, "eventual.exists"),
    });
    const getMany = (keys) => Effect.gen(function* () {
        const raw = yield* Effect.try({
            try: () => KvBatch.getMany(handle, [...keys]),
            catch: (cause) => new KeyValueHostError(cause, "eventual-batch.get-many"),
        });
        const out = [];
        for (const iv of raw) {
            if (iv === undefined || iv === null) {
                out.push(Option.none());
            }
            else {
                const bytes = yield* consumeIncoming(iv);
                out.push(Option.some(bytes));
            }
        }
        return out;
    });
    const setMany = (entries) => Effect.gen(function* () {
        const pairs = [];
        for (const [k, v] of entries) {
            const ov = yield* writeOutgoing(v);
            pairs.push([k, ov]);
        }
        yield* Effect.try({
            try: () => KvBatch.setMany(handle, pairs),
            catch: (cause) => new KeyValueHostError(cause, "eventual-batch.set-many"),
        });
    });
    const deleteMany = (keys) => Effect.try({
        try: () => KvBatch.deleteMany(handle, [...keys]),
        catch: (cause) => new KeyValueHostError(cause, "eventual-batch.delete-many"),
    });
    return {
        name,
        get,
        set,
        delete: del,
        exists,
        getMany,
        setMany,
        deleteMany,
        get keys() {
            return Effect.try({
                try: () => KvBatch.keys(handle),
                catch: (cause) => new KeyValueHostError(cause, "eventual-batch.keys"),
            });
        },
    };
};
const liveOpenBucket = (name) => Effect.gen(function* () {
    const handle = yield* Effect.acquireRelease(Effect.try({
        try: () => KvTypes.Bucket.openBucket(name),
        catch: (cause) => new KeyValueHostError(cause, "openBucket"),
    }), 
    // The WIT bucket resource has no .close() method; rely on JS GC
    // when the scope's reference is dropped.
    () => Effect.void);
    return makeHostBucket(name, handle);
});
const KeyValueLive = Layer.succeed(KeyValueClient, KeyValueClient.of({
    openBucket: liveOpenBucket,
}));

/**
 * Host service for `wasi:logging/logging`. Wraps the host's
 * `log(level, context, message)` call so SDK code can emit log lines
 * via Effect-typed DI rather than a direct ESM specifier import.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class LoggingHost extends Context.Service()("effect-golem/host/Logging") {
}
const LoggingHostLive = Layer.succeed(LoggingHost, LoggingHost.of({
    log: (level, context, message) => WasiLogging.log(level, context, message),
}));

/**
 * Host service for `golem:rdbms/mysql@1.5.0`. Wraps the synchronous
 * `DbConnection.open(address)` static entry point so SDK code can
 * acquire a connection via DI rather than reaching into the imported
 * namespace directly.
 *
 * Per the layer-mocks refactor plan §4c, the `DbConnection`,
 * `DbTransaction`, and `DbResultStream` resources themselves are NOT
 * services — only the constructor / entry point that produces them is.
 * Their methods (`query`, `execute`, `queryStream`, `beginTransaction`,
 * `commit`, `rollback`, …) are still synchronous and called from
 * `src/mysql.ts` inside `Effect.try` wrappers (see the surrounding
 * adapter for error classification).
 *
 * The user-facing `MySqlClient` adapter (`src/mysql.ts`) keeps its
 * byte-compatible public surface; the only change is that
 * `MySqlClient.make` / `MySqlClient.layer` now require
 * {@link MysqlHostClient} to be provided upstream. The agent
 * dispatcher's `provideUserRuntime` helper provides `HostLive` (which
 * contains {@link MysqlHostLive}), so user code inside `defineAgent`
 * impls never sees the requirement.
 *
 * Naming: `MysqlHostClient` (with the `HostClient` suffix) is
 * deliberately distinct from the user-facing `MySqlClient` adapter
 * that implements `effect/sql/SqlClient`.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class MysqlHostClient extends Context.Service()("effect-golem/host/Mysql") {
}
const MysqlHostLive = Layer.succeed(MysqlHostClient, MysqlHostClient.of({
    open: (address) => DbConnection$1.open(address),
}));

/**
 * Host service wrapping the `node:sqlite` `DatabaseSync` constructor.
 *
 * `node:sqlite` is a Node built-in (and is exposed verbatim by Golem's
 * `wasm-rquickjs` runtime), so the only piece we need to abstract is
 * the `new DatabaseSync(filename, opts?)` call: routing it through a
 * tagged service lets tests substitute a fake `DatabaseSync` factory
 * via `Layer.succeed(NodeSqliteClient, …)` instead of monkey-patching
 * the imported namespace.
 *
 * The handle returned by {@link NodeSqliteClientShape.open} is exposed
 * verbatim — its `prepare` / `exec` / etc. methods are still called
 * synchronously by the consumer (`src/sqlite.ts`). Only the
 * acquire-release lifecycle is Effect-typed, so the surrounding scope
 * deterministically closes the handle.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class NodeSqliteClient extends Context.Service()("effect-golem/host/NodeSqlite") {
}
const NodeSqliteLive = Layer.succeed(NodeSqliteClient, NodeSqliteClient.of({
    open: (filename, opts) => Effect.acquireRelease(Effect.try({
        try: () => new DatabaseSync(filename, { readOnly: opts?.readOnly ?? false }),
        catch: (cause) => cause,
    }), (db) => Effect.sync(() => {
        try {
            db.close();
        }
        catch {
            /* idempotent — close() throws if already closed */
        }
    })),
}));

/**
 * Host service bundling the agent's oplog accessors:
 *
 * - `golem:api/oplog@1.5.0` — `enrichOplogEntries` plus the
 *   `GetOplog` and `SearchOplog` resource constructors used to drive
 *   paged reads of an agent's oplog.
 *
 * The cursor pagers are exposed as scoped `Effect`s — the host's
 * `GetOplog` / `SearchOplog` resources are dropped when the surrounding
 * `Scope` closes (the WIT contract says "no-op on already-dropped"; we
 * rely on JS GC since `wasi:io`-style explicit `[resource-drop]` is
 * not exposed for these classes), so consumers do not have to remember
 * to release the handle manually.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class OplogClient extends Context.Service()("effect-golem/host/Oplog") {
}
const OplogLive = Layer.succeed(OplogClient, OplogClient.of({
    enrichOplogEntries: (environmentId, agentId, entries, componentRevision) => OplogHost.enrichOplogEntries(environmentId, agentId, entries.map(([i, e]) => [i, e]), componentRevision),
    newGetOplog: (agentId, start) => {
        const handle = new OplogHost.GetOplog(agentId, start);
        return {
            getNext: () => {
                const out = handle.getNext();
                return out === undefined ? undefined : [...out];
            },
        };
    },
    newSearchOplog: (agentId, text) => {
        const handle = new OplogHost.SearchOplog(agentId, text);
        return {
            getNext: () => {
                const out = handle.getNext();
                return out === undefined
                    ? undefined
                    : out.map(([i, e]) => [i, e]);
            },
        };
    },
}));

class PermissionsClient extends Context.Service()("effect-golem/host/Permissions") {
}
const PermissionsLive = Layer.succeed(PermissionsClient, PermissionsClient.of({
    id: TypesHost.id,
    parents: TypesHost.parents,
    expiresAt: TypesHost.expiresAt,
    inspect: InspectHost.inspectCard,
    derive: DeriveHost.derive,
    deriveFromWallet: DeriveHost.deriveFromWallet,
    deriveScope: DeriveHost.deriveScope,
    revoke: RevokeHost.revokeCard,
    wallet: WalletHost.selfWallet,
    walletVersion: WalletHost.selfVersion,
    install: WalletHost.installCard,
    listModules: KernelHost.listModules,
    validateGrant: KernelHost.validateGrant,
    drop: (card) => {
        card[Symbol.dispose]?.();
    },
}));

/**
 * Host service for `golem:rdbms/postgres@1.5.0`. Wraps the synchronous
 * `DbConnection.open(address)` static entry point so SDK code can
 * acquire a connection via DI rather than reaching into the imported
 * namespace directly.
 *
 * Per the layer-mocks refactor plan §4c, the `DbConnection`,
 * `DbTransaction`, and `DbResultStream` resources themselves are NOT
 * services — only the constructor / entry point that produces them is.
 * Their methods (`query`, `execute`, `queryStream`, `beginTransaction`,
 * `commit`, `rollback`, …) are still synchronous and called from
 * `src/postgres.ts` inside `Effect.try` wrappers (see the surrounding
 * adapter for error classification).
 *
 * The user-facing `PgClient` adapter (`src/postgres.ts`) keeps its
 * byte-compatible public surface; the only change is that
 * `PgClient.make` / `PgClient.layer` now require {@link PostgresHostClient}
 * to be provided upstream. The agent dispatcher's `provideUserRuntime`
 * helper provides `HostLive` (which contains {@link PostgresHostLive}),
 * so user code inside `defineAgent` impls never sees the requirement.
 *
 * Naming: `PostgresHostClient` (with the `HostClient` suffix) is
 * deliberately distinct from the user-facing `PgClient` adapter that
 * implements `effect/sql/SqlClient`.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class PostgresHostClient extends Context.Service()("effect-golem/host/Postgres") {
}
const PostgresHostLive = Layer.succeed(PostgresHostClient, PostgresHostClient.of({
    open: (address) => DbConnection$2.open(address),
}));

/**
 * Host service for the promise-rendezvous subset of
 * `golem:api/host@1.5.0`:
 *
 * - `createPromise` — allocate a new host promise.
 * - `getPromise` — open a `GetPromiseResult` handle for a promise id.
 * - `completePromise` — complete a promise with a payload.
 *
 * Used by `src/Agents.ts` (`Promises.create` / `await` /
 * `complete`) and by `src/Webhook.ts` (which composes
 * `createPromise` with `golem:agent/host.createWebhook`).
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class PromiseClient extends Context.Service()("effect-golem/host/Promise") {
}
const PromiseLive = Layer.succeed(PromiseClient, PromiseClient.of({
    createPromise: () => ApiHost.createPromise(),
    getPromise: (id) => ApiHost.getPromise(id),
    completePromise: (id, payload) => ApiHost.completePromise(id, payload),
}));

/**
 * Host service for `golem:api/retry@1.5.0`. Wraps the synchronous host
 * functions (`get-retry-policies`, `get-retry-policy-by-name`,
 * `resolve-retry-policy`, `set-retry-policy`, `remove-retry-policy`) and
 * the `golem:api/retry-scope@1.5.0` resource into an Effect-typed surface so SDK code can reach them via DI rather
 * than importing the WIT specifier directly.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class RetryClient extends Context.Service()("effect-golem/host/Retry") {
}
const RetryLive = Layer.succeed(RetryClient, RetryClient.of({
    nextRetryDelay: (...args) => RetryHost.nextRetryDelay(...args),
    now: () => process.hrtime.bigint(),
    getRetryPolicies: () => RetryHost.getRetryPolicies(),
    getRetryPolicyByName: (name) => RetryHost.getRetryPolicyByName(name),
    resolveRetryPolicy: (verb, nounUri, properties) => RetryHost.resolveRetryPolicy(verb, nounUri, properties.map(([k, v]) => [k, v])),
    setRetryPolicy: (policy) => RetryHost.setRetryPolicy(policy),
    removeRetryPolicy: (name) => RetryHost.removeRetryPolicy(name),
    openRetryPolicyScope: (policy) => {
        const scope = new RetryScopeHost.RetryPolicyScope(policy);
        return () => scope[Symbol.dispose]();
    },
}));

/**
 * Host service wrapping Golem's three `wasm-rquickjs` extensions to
 * Node's built-in `node:sqlite` module:
 *
 * - `serializeDatabaseSync(db) -> Uint8Array`
 * - `restoreDatabaseSync(db, bytes) -> void`
 * - `isAutocommitDatabaseSync(db) -> boolean`
 *
 * These do not exist on stock Node, so the production layer delegates
 * to the same `node:sqlite` namespace import the SDK already used; the
 * `wasm-rquickjs` runtime surfaces them as additional exports on that
 * specifier. Tests substitute fakes via
 * `Layer.succeed(SqliteHostExtClient, …)`.
 *
 * The shape methods are intentionally synchronous (no Effect wrap) so
 * the snapshot dispatcher's existing imperative loops in
 * {@link ../agent.dispatchSaveSnapshot} /
 * {@link ../agent.dispatchLoadSnapshot} can keep their structure: the
 * dispatcher reads the impl out of the user runtime layer once at the
 * top via `Effect.runPromise`, then calls the methods directly inside
 * the synchronous loop. Mirrors the `parseAgentId` / `getSelfMetadata`
 * pattern on {@link AgentHostClient}.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class SqliteHostExtClient extends Context.Service()("effect-golem/host/SqliteHostExt") {
}
const SqliteHostExtLive = Layer.succeed(SqliteHostExtClient, SqliteHostExtClient.of({
    serializeDatabaseSync: (db) => NodeSqlite.serializeDatabaseSync(db),
    restoreDatabaseSync: (db, bytes) => NodeSqlite.restoreDatabaseSync(db, bytes),
    isAutocommitDatabaseSync: (db) => NodeSqlite.isAutocommitDatabaseSync(db),
}));

/** Injectable transport for the ambient Golem tool host. @since 1.6.0 @category host services */
class ToolClient extends Context.Service()("effect-golem/host/ToolClient") {
}
/** Live tool-host layer. @since 1.6.0 @category layers */
const ToolClientLive = Layer.succeed(ToolClient, ToolClient.of({
    getAllTools: Host$1.getAllTools,
    getTool: Host$1.getTool,
    createStdin: Host$1.createStdin,
    createStdinFromStream: Host$1.createStdinFromStream,
    createOutput: Host$1.createOutput,
    rpc: (name) => new Host$1.ToolRpc(name),
    createRpc: (name) => Host$1.ToolRpc.create(name),
}));

/**
 * Host service for `golem:api/context@1.5.0`. Wraps `startSpan`,
 * `currentContext`, and `allowForwardingTraceContextHeaders` into an
 * Effect-typed surface so SDK code can interact with the host's
 * tracing context via DI rather than a direct ESM specifier import.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class TracingHost extends Context.Service()("effect-golem/host/Tracing") {
}
const TracingHostLive = Layer.succeed(TracingHost, TracingHost.of({
    startSpan: (name) => ContextHost.startSpan(name),
    currentContext: () => ContextHost.currentContext(),
    allowForwardingTraceContextHeaders: (allow) => ContextHost.allowForwardingTraceContextHeaders(allow),
}));

/**
 * Host service for `golem:websocket/client@1.5.0`. Wraps the WIT
 * `WebsocketConnection` resource constructor (`connect(url, headers)`)
 * so SDK code can acquire a connection via Effect-typed DI rather
 * than reaching directly into the imported namespace.
 *
 * The synchronous `connect` returns a live `WebsocketConnection`
 * resource whose `send` / `receive` / `subscribe` / `close` methods
 * are exercised directly by `src/websocket.ts`. The lifecycle
 * (`Effect.acquireRelease` calling `ws.close(1000, undefined)` on
 * scope close) lives in the consumer; the service only abstracts the
 * constructor call.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
class WebsocketClient extends Context.Service()("effect-golem/host/Websocket") {
}
const WebsocketLive = Layer.succeed(WebsocketClient, WebsocketClient.of({
    connect: (url, headers, policy) => WsClient.WebsocketConnection.connect(url, headers, policy),
}));

/**
 * Top-level merge of every per-WIT-interface "Live" host layer. Built
 * once at module load (consumed by the agent dispatcher's
 * `provideUserRuntime` helper) and shared across every dispatch entry
 * (`initialize`, `invoke`, `save-snapshot`, `load-snapshot`).
 *
 * Each constituent layer is a thin wrapper around a single WIT
 * specifier import. Tests substitute fakes via
 * `Layer.succeed(Tag, fake)` / `Effect.provide(eff, fakeLayer)`.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
const HostLive = Layer.mergeAll(EnvironmentLive, ConfigLive, AgentHostLive, InvocationContextLive, PromiseLive, DurabilityLive, CustomInvocationContextLive, DurabilityModeLive, DurableStreamsLive, OplogLive, RpcLive, SecretsLive, ToolClientLive, LoggingHostLive, TracingHostLive, QuotaLive, PermissionsLive, RetryLive, NodeSqliteLive, SqliteHostExtLive, PostgresHostLive, MysqlHostLive, IgniteHostLive, WebsocketLive, BlobstoreLive, KeyValueLive);

// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1
/** Copy ergonomic file response headers into their ordered WIT representation. */
function compileFileResponseHeaders(headers) {
    return Object.entries(headers ?? {}).map(([name, value]) => ({
        name,
        value,
    }));
}
/** Safe diagnostics never contain document, header, or body contents. */
class HttpRouterError extends TypeError {
    category;
    constructor(category) {
        super(category);
        this.category = category;
        this.name = 'HttpRouterError';
    }
}
function fail(category) {
    throw new HttpRouterError(category);
}
function validUnicode(value) {
    for (const char of value) {
        const code = char.codePointAt(0);
        if (code >= 0xd800 && code <= 0xdfff)
            return false;
    }
    return true;
}
function validSegment(value) {
    return (value !== '' &&
        value !== '.' &&
        value !== '..' &&
        !/[\x00-\x1f\x7f/\\]/.test(value) &&
        validUnicode(value));
}
function publicSegments(path) {
    if (path === '/')
        return [];
    if (!path.startsWith('/') || /[$*?#]/.test(path))
        fail('source-path');
    return path
        .slice(1)
        .split('/')
        .map((raw) => {
        if (!/^(?:[A-Za-z0-9\-._~!&'()+,;=:@]|%[A-Fa-f0-9]{2})+$/.test(raw))
            fail('source-path');
        let segment;
        try {
            segment = decodeURIComponent(raw);
        }
        catch {
            return fail('source-path');
        }
        if (!validSegment(segment))
            fail('source-path');
        return segment;
    });
}
/** Compile once to WIT mappings. Only identical compiled pairs are duplicates. */
function compileFileMappings(mappings) {
    const seen = new Set();
    return mappings.map(({ route, path }) => {
        const subtree = route.endsWith('/*');
        const source = subtree ? route.slice(0, -2) || '/' : route;
        if (source.includes('*'))
            fail('source-wildcard');
        if (subtree && source !== '/' && source.endsWith('/'))
            fail('source-path');
        // `//*` is not the root wildcard spelling.
        if (subtree && route !== '/*' && source === '/')
            fail('source-path');
        const segments = publicSegments(source);
        if (subtree && !path.endsWith('/$1'))
            fail('target-placeholder');
        const target = subtree ? path.slice(0, -3) || '/' : path;
        if (target.includes('$'))
            fail('target-placeholder');
        if (!target.startsWith('/') ||
            (!subtree && target === '/') ||
            /[\x00-\x1f\x7f-\x9f]/.test(target) ||
            !validUnicode(target) ||
            (target !== '/' && !target.slice(1).split('/').every(validSegment)) ||
            (subtree && path !== '/$1' && target === '/'))
            fail('target-path');
        const mapping = subtree
            ? {
                tag: 'subtree',
                val: { publicPrefix: segments, filesystemRoot: target },
            }
            : { tag: 'exact', val: { publicPath: segments, filePath: target } };
        const key = JSON.stringify(mapping);
        if (seen.has(key))
            fail('duplicate-mapping');
        seen.add(key);
        return mapping;
    });
}

const verbToWit = (v) => {
    if (typeof v !== "string")
        return { tag: "method", val: { tag: "custom", val: v.custom } };
    switch (v) {
        case "GET":
            return { tag: "method", val: { tag: "get" } };
        case "HEAD":
            return { tag: "method", val: { tag: "head" } };
        case "POST":
            return { tag: "method", val: { tag: "post" } };
        case "PUT":
            return { tag: "method", val: { tag: "put" } };
        case "DELETE":
            return { tag: "method", val: { tag: "delete" } };
        case "CONNECT":
            return { tag: "method", val: { tag: "connect" } };
        case "OPTIONS":
            return { tag: "method", val: { tag: "options" } };
        case "TRACE":
            return { tag: "method", val: { tag: "trace" } };
        case "PATCH":
            return { tag: "method", val: { tag: "patch" } };
    }
};
/** Verbs that may not carry a request body (per the SDK convention). */
const isBodylessVerb = (v) => typeof v === "string" && (v === "GET" || v === "HEAD" || v === "DELETE");
// ---------------------------------------------------------------------------
// Typed failures
// ---------------------------------------------------------------------------
/**
 * Typed failure surfaced by the parser and validator. Threaded into
 * `registerAgent`'s Effect alongside `UnsupportedSchemaError`.
 *
 * @since 1.5.0
 * @category errors
 */
class HttpRouteError {
    reason;
    _tag = "HttpRouteError";
    constructor(reason) {
        this.reason = reason;
    }
}
// ---------------------------------------------------------------------------
// Compilation: MountDef / EndpointDef → WIT records
// ---------------------------------------------------------------------------
const segmentToWit = (s) => {
    switch (s._tag) {
        case "Literal":
            return { tag: "segment", val: { tag: "literal", val: s.value } };
        case "PathVar":
            return { tag: "segment", val: { tag: "path-variable", val: { variableName: s.name } } };
        case "RestVar":
            return {
                tag: "segment",
                val: { tag: "remaining-path-variable", val: { variableName: s.name } },
            };
        case "SystemVar":
            return { tag: "system-variable", val: s.name };
    }
};
/**
 * Compile a {@link MountDef} to the WIT `http-mount-details` record.
 *
 * @since 1.5.0
 * @category metadata
 */
const compileMount = (mountDef) => ({
    pathPrefix: mountDef.pathPrefix.map(segmentToWit),
    authDetails: mountDef.authRequired ? { required: true } : undefined,
    phantomAgent: mountDef.phantomAgent,
    phantomIdBinding: mountDef.phantomId
        ? {
            tag: mountDef.phantomId.source,
            val: { name: mountDef.phantomId.name, optional: mountDef.phantomId.optional },
        }
        : undefined,
    corsOptions: { allowedPatterns: [...mountDef.cors] },
    webhookSuffix: mountDef.webhookSuffix.map(segmentToWit),
    staticBindings: [],
    filesystemBindings: compileFileMappings(mountDef.exposeFiles ?? []),
    openapiProviderMethod: undefined,
    fileResponseHeaders: compileFileResponseHeaders(mountDef.fileResponseHeaders),
});
/**
 * Compile a single {@link EndpointDef} to a WIT `http-endpoint-details` record.
 *
 * @since 1.5.0
 * @category metadata
 */
const compileEndpoint = (ep) => {
    if (ep.mode !== undefined &&
        ep.mode !== "durableStreams" &&
        ep.mode !== "plain" &&
        ep.mode !== "sse" &&
        ep.mode !== "webSocket") {
        throw new TypeError("mode must be durableStreams, plain, sse, or webSocket");
    }
    return {
        httpMethod: verbToWit(ep.verb),
        pathSuffix: ep.pathSuffix.map(segmentToWit),
        headerVars: ep.headerVars.map((h) => ({
            headerName: h.header,
            variableName: h.varName,
        })),
        queryVars: ep.queryVars.map((q) => ({
            queryParamName: q.queryParam,
            variableName: q.varName,
        })),
        authDetails: ep.authRequired === undefined ? undefined : { required: ep.authRequired },
        corsOptions: { allowedPatterns: [...ep.cors] },
        mode: ep.mode === "durableStreams"
            ? "durable-streams"
            : ep.mode === "webSocket"
                ? "web-socket"
                : ep.mode,
        durableStreams: ep.durableStreams === undefined
            ? undefined
            : {
                slots: ep.durableStreams.slots.map((slot) => ({
                    source: { tag: slot.source, val: slot.slot },
                    name: slot.name,
                    contentType: slot.contentType,
                })),
                load: ep.durableStreams.load === undefined
                    ? undefined
                    : {
                        maxConcurrentReadersPerStream: ep.durableStreams.load.maxConcurrentReadersPerStream,
                        maxAppendRequestsPerSecondPerStream: ep.durableStreams.load.maxAppendRequestsPerSecondPerStream,
                    },
            },
    };
};
const lowerCase = (s) => s.toLowerCase();
const validateEndpoint = (agentName, m, ep, endpointIndex) => Effect.gen(function* () {
    const ctx = `agent '${agentName}' method '${m.name}' endpoint #${endpointIndex}`;
    const seenSources = new Map();
    // Path-suffix variables.
    let restCount = 0;
    for (let i = 0; i < ep.pathSuffix.length; i++) {
        const seg = ep.pathSuffix[i];
        if (seg._tag === "Literal" || seg._tag === "SystemVar")
            continue;
        if (seg._tag === "RestVar") {
            restCount += 1;
            if (i !== ep.pathSuffix.length - 1) {
                return yield* Effect.fail(new HttpRouteError(`${ctx}: catch-all '{*${seg.name}}' is only allowed as the LAST path segment`));
            }
        }
        const name = seg.name;
        if (!(name in m.params)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: path variable '${name}' does not match any method parameter (params: ${Object.keys(m.params).join(", ") || "<none>"})`));
        }
        if (m.nonStringBindableParams.has(name)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: parameter '${name}' is multimodal/unstructured and cannot be bound from a path variable`));
        }
        if (!m.stringBindableParams.has(name)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: parameter '${name}' has a schema that is not bindable from a path variable (only String, Number, BigInt, Boolean, Literal, or branded variants thereof are supported)`));
        }
        const prev = seenSources.get(name);
        if (prev !== undefined) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: parameter '${name}' is bound from both '${prev}' and 'path'`));
        }
        seenSources.set(name, "path");
    }
    if (restCount > 1) {
        return yield* Effect.fail(new HttpRouteError(`${ctx}: more than one catch-all variable in path`));
    }
    // Query variables.
    const seenQueryKeys = new Set();
    for (const q of ep.queryVars) {
        if (seenQueryKeys.has(q.queryParam)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: duplicate query parameter '${q.queryParam}'`));
        }
        seenQueryKeys.add(q.queryParam);
        if (!(q.varName in m.params)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: query variable '${q.varName}' (from query parameter '${q.queryParam}') does not match any method parameter`));
        }
        if (m.nonStringBindableParams.has(q.varName)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: parameter '${q.varName}' is multimodal/unstructured and cannot be bound from a query parameter`));
        }
        if (!m.queryBindableParams.has(q.varName)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: parameter '${q.varName}' has a schema that is not bindable from a query parameter (only primitive schemas, branded variants thereof, or arrays of those primitives are supported)`));
        }
        const prev = seenSources.get(q.varName);
        if (prev !== undefined) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: parameter '${q.varName}' is bound from both '${prev}' and 'query'`));
        }
        seenSources.set(q.varName, "query");
    }
    // Header variables (case-insensitive uniqueness).
    const seenHeaderKeys = new Set();
    for (const h of ep.headerVars) {
        const key = lowerCase(h.header);
        if (seenHeaderKeys.has(key)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: duplicate header '${h.header}' (case-insensitive)`));
        }
        seenHeaderKeys.add(key);
        if (!(h.varName in m.params)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: header variable '${h.varName}' (from header '${h.header}') does not match any method parameter`));
        }
        if (m.nonStringBindableParams.has(h.varName)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: parameter '${h.varName}' is multimodal/unstructured and cannot be bound from a header`));
        }
        if (!(m.queryOrHeaderBindableParams ?? m.stringBindableParams).has(h.varName)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: parameter '${h.varName}' has a schema that is not bindable from a header (only primitive schemas, branded variants thereof, or arrays of those primitives are supported)`));
        }
        const prev = seenSources.get(h.varName);
        if (prev !== undefined) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: parameter '${h.varName}' is bound from both '${prev}' and 'header'`));
        }
        seenSources.set(h.varName, "header");
    }
    // Inference belongs to the registry compiler; validate without adding templates.
    if (isBodylessVerb(ep.verb)) {
        for (const paramName of Object.keys(m.params)) {
            if (!seenSources.has(paramName) &&
                !m.streamParams.has(paramName) &&
                !m.queryBindableParams.has(paramName)) {
                return yield* Effect.fail(new HttpRouteError(`${ctx}: ${verbLabel(ep.verb)} cannot bind implicit query input '${paramName}': expected a scalar, scalar list, scalar map, or outer option`));
            }
        }
    }
});
const verbLabel = (v) => (typeof v === "string" ? v : `custom('${v.custom}')`);
/**
 * Validate a complete agent's HTTP route metadata, producing the
 * compiled WIT records for everything that passes. Errors surface as
 * `HttpRouteError` typed failures.
 *
 * @since 1.5.0
 * @category metadata
 */
const validateAgentHttp = (input) => Effect.gen(function* () {
    const anyEndpoints = input.methods.some((m) => m.endpoints.length > 0);
    // Mount must exist when any endpoint is declared.
    if (anyEndpoints && input.mount === undefined) {
        return yield* Effect.fail(new HttpRouteError(`agent '${input.agentName}' declares HTTP endpoints but no mount — add an 'http: Http.mount(...)' to the agent definition`));
    }
    // Mount validation.
    let compiledMount = undefined;
    if (input.mount !== undefined) {
        compiledMount = yield* validateMount(input);
    }
    // Per-method endpoints.
    const endpoints = new Map();
    for (const m of input.methods) {
        const compiled = [];
        for (let i = 0; i < m.endpoints.length; i++) {
            yield* validateEndpoint(input.agentName, m, m.endpoints[i], i);
            compiled.push(compileEndpoint(m.endpoints[i]));
        }
        endpoints.set(m.name, compiled);
    }
    return { mount: compiledMount, endpoints };
});
const validateMount = (input) => Effect.gen(function* () {
    const mountDef = input.mount;
    const ctx = `agent '${input.agentName}' mount`;
    const selector = mountDef.phantomId;
    if (selector) {
        if (typeof selector.optional !== "boolean") {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: phantom selector optionality must be a boolean`));
        }
        if (!selector.name ||
            mountDef.exposeFiles?.length ||
            (selector.source === "path" &&
                (input.constructorParamNames.includes(selector.name) ||
                    mountDef.pathPrefix.filter((s) => s._tag === "PathVar" && s.name === selector.name)
                        .length !== 1))) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: invalid phantom selector ownership or file exposure`));
        }
        for (const m of input.methods) {
            for (const ep of m.endpoints) {
                if ((selector.source === "path" &&
                    ep.pathSuffix.some((s) => (s._tag === "PathVar" || s._tag === "RestVar") && s.name === selector.name)) ||
                    (selector.source === "query" &&
                        (ep.queryVars.some((q) => q.queryParam === selector.name) ||
                            (ep.durableStreams && ["offset", "cursor", "live"].includes(selector.name))))) {
                    return yield* Effect.fail(new HttpRouteError(`${ctx}: phantom selector conflicts with an endpoint binding or durable-stream control`));
                }
            }
        }
    }
    const mountVars = new Set();
    for (let i = 0; i < mountDef.pathPrefix.length; i++) {
        const s = mountDef.pathPrefix[i];
        if (s._tag === "Literal" || s._tag === "SystemVar")
            continue;
        if (s._tag === "RestVar") {
            // Already rejected by the parser, but defend in depth in case
            // callers built a MountDef structurally.
            return yield* Effect.fail(new HttpRouteError(`${ctx}: mount path may not include catch-all '{*${s.name}}'`));
        }
        if (mountVars.has(s.name)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: duplicate path variable '${s.name}'`));
        }
        mountVars.add(s.name);
        if (selector?.source === "path" && selector.name === s.name)
            continue;
        if (!input.constructorParamNames.includes(s.name)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: path variable '${s.name}' does not match any constructor parameter (constructorParams: ${input.constructorParamNames.join(", ") || "<none>"})`));
        }
        if (input.nonStringBindableConstructorParams.has(s.name)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: constructor parameter '${s.name}' is multimodal/unstructured and cannot be bound from a path variable`));
        }
        if (!input.stringBindableConstructorParams.has(s.name)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: constructor parameter '${s.name}' has a schema that is not bindable from a path variable (only String, Number, BigInt, Boolean, Literal, or branded variants thereof are supported)`));
        }
    }
    // Every constructor param must be covered by a mount path variable.
    for (const cp of input.constructorParamNames) {
        if (!mountVars.has(cp)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: constructor parameter '${cp}' is not covered by a mount path variable — add '{${cp}}' to the mount path`));
        }
    }
    // Webhook suffix variables, if any, must:
    //   (a) be unique within the suffix,
    //   (b) match a constructor parameter,
    //   (c) NOT refer to a multimodal / unstructured constructor param,
    //   (d) be on a string-bindable schema (string / number / bigint /
    //       boolean / literal / branded variants thereof).
    // Mirrors the mount-path checks above so the rules are consistent
    // between the routable mount path and the deploy-time webhook URL.
    const webhookVars = new Set();
    for (const s of mountDef.webhookSuffix) {
        if (s._tag !== "PathVar")
            continue;
        if (webhookVars.has(s.name)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: duplicate webhook-suffix path variable '${s.name}'`));
        }
        webhookVars.add(s.name);
        if (!input.constructorParamNames.includes(s.name)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: webhook-suffix path variable '${s.name}' does not match any constructor parameter`));
        }
        if (input.nonStringBindableConstructorParams.has(s.name)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: webhook-suffix constructor parameter '${s.name}' is multimodal/unstructured and cannot be bound from a path variable`));
        }
        if (!input.stringBindableConstructorParams.has(s.name)) {
            return yield* Effect.fail(new HttpRouteError(`${ctx}: webhook-suffix constructor parameter '${s.name}' has a schema that is not bindable from a path variable (only String, Number, BigInt, Boolean, Literal, or branded variants thereof are supported)`));
        }
    }
    return compileMount(mountDef);
});
// ---------------------------------------------------------------------------
// Schema-side helper used by `agent.ts` / `method.ts`
// ---------------------------------------------------------------------------
/**
 * Walk a {@link Schema.Top}'s AST to determine whether values of this
 * schema can be safely decoded from a string source (URL path, query
 * parameter, or header). Returns `true` for `Schema.String`,
 * `Schema.Number`, `Schema.BigInt`, `Schema.Boolean`, literal/enum
 * schemas, and any refinements/transformations layered on top of them.
 *
 * @since 1.5.0
 * @category guards
 */
const isStringBindableSchema = (schema) => isStringBindableAst(schema.ast);
/**
 * Determine whether a schema can be decoded from query/header values.
 * In addition to the scalar schemas accepted by
 * {@link isStringBindableSchema}, the host supports one plain
 * `list<primitive>` whose values come from repeated query parameters
 * or repeated header instances.
 *
 * @since 1.5.1
 * @category guards
 */
const isQueryOrHeaderBindableSchema = (schema) => isStringBindableAst(schema.ast) ||
    isOptionalStringBindableAst(schema.ast) ||
    isPrimitiveArrayAst(schema.ast);
const isPrimitiveArrayAst = (ast) => {
    if (ast._tag !== "Arrays")
        return false;
    const array = ast;
    return (array.elements?.length === 0 &&
        array.rest?.length === 1 &&
        isCollectionElementBindableAst(array.rest[0]));
};
const isOptionalStringBindableAst = (ast) => {
    if (ast._tag !== "Union")
        return false;
    const union = ast;
    if (union.types === undefined)
        return false;
    const emptyMembers = union.types.filter(isNullOrUndefinedAst);
    const realMembers = union.types.filter((member) => !isNullOrUndefinedAst(member));
    return (emptyMembers.length === 1 && realMembers.length === 1 && isStringBindableAst(realMembers[0]));
};
const isNullOrUndefinedAst = (ast) => ast._tag === "Null" || ast._tag === "Undefined" || ast._tag === "Void";
const isCollectionElementBindableAst = (ast) => {
    switch (ast._tag) {
        case "String":
        case "Number":
        case "BigInt":
        case "Boolean":
        case "TemplateLiteral":
        case "Enum":
            return true;
        case "Literal": {
            const literal = ast.literal;
            return (typeof literal === "string" ||
                typeof literal === "number" ||
                typeof literal === "bigint" ||
                typeof literal === "boolean");
        }
        case "Union": {
            const union = ast;
            return (union.types !== undefined &&
                union.types.length > 0 &&
                union.types.every((member) => member._tag === "Literal" && typeof member.literal === "string"));
        }
    }
    const encoded = ast;
    if (encoded.from && typeof encoded.from._tag === "string") {
        return isCollectionElementBindableAst(encoded.from);
    }
    if (encoded.to && typeof encoded.to._tag === "string") {
        return isCollectionElementBindableAst(encoded.to);
    }
    return false;
};
const isStringBindableAst = (ast) => {
    switch (ast._tag) {
        case "String":
        case "Number":
        case "BigInt":
        case "Boolean":
            return true;
        case "Literal":
            // string/number/boolean/bigint literals are all valid in URL
            // contexts; null literal also serialisable.
            return true;
        case "Enum":
            return true;
        case "TemplateLiteral":
            return true;
        case "UniqueSymbol":
            return false;
        case "Union": {
            const u = ast;
            if (u.types)
                return u.types.every(isStringBindableAst);
            return false;
        }
    }
    // Refinements / suspends / transformations: peel one layer.
    const a = ast;
    if (a.from && typeof a.from._tag === "string") {
        return isStringBindableAst(a.from);
    }
    if (a.to && typeof a.to._tag === "string") {
        return isStringBindableAst(a.to);
    }
    return false;
};

/**
 * @since 1.5.0
 */
/**
 * Read the running agent's own structured
 * {@link CoreTypes.AgentId}.
 *
 * Each execution reads durable host metadata without caching. During
 * replay it reproduces the identity observed at that point in history;
 * new live reads after a fork identify the child, even within the same
 * invocation. Use `yield* SelfAgentId.SelfAgentId` in initialization,
 * methods, or snapshot effects. Values explicitly saved in application
 * state are ordinary state and are not rebound after a fork.
 *
 * For richer fields (component revision, status, retry count, env,
 * config), use `Agents.getSelfMetadata` directly.
 *
 * Requires view permission on the running agent because it reads full
 * self metadata before extracting the identity. Pure agent initialization
 * does not execute this effect. Restoration with an explicit snapshot
 * strategy, including `Snapshot.ref()`, does, to populate the restoration
 * context's `agentId`. Plain schema-state restoration without a strategy
 * does not execute this effect.
 *
 * Outside the dispatcher (e.g. unit tests), provide `AgentHostClient`
 * with a host implementation. Host failures become Effect defects.
 *
 * @since 1.5.0
 * @category host services
 */
const SelfAgentId = Effect.gen(function* () {
    const host = yield* AgentHostClient;
    return yield* Effect.sync(() => host.getSelfMetadata().agentId);
});

/** Byte-preserving multipart/mixed framing shared by snapshot modes. @internal @since 1.5.0 */
const encoder$1 = new TextEncoder();
const decoder = new TextDecoder();
const CRLF = "\r\n";
/** Malformed multipart framing or headers. @since 1.5.0 @category errors */
class MultipartCodecError extends Error {
    reason;
    _tag = "MultipartCodecError";
    constructor(reason) {
        super(reason);
        this.reason = reason;
        this.name = "MultipartCodecError";
    }
}
const validBoundary = (boundary) => /^[A-Za-z0-9'()+_,./:=?-]{1,70}$/.test(boundary);
const matches = (data, text, offset) => {
    if (offset < 0 || offset + text.length > data.length)
        return false;
    for (let i = 0; i < text.length; i++) {
        if (data[offset + i] !== text[i])
            return false;
    }
    return true;
};
const concat = (chunks) => {
    const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
    let offset = 0;
    for (const chunk of chunks) {
        out.set(chunk, offset);
        offset += chunk.length;
    }
    return out;
};
// Returns the suffix position only for complete delimiter lines.
const delimiterAt = (data, marker, newline, offset) => {
    if (!matches(data, marker, offset))
        return undefined;
    let end = offset + marker.length;
    const closing = data[end] === 45 && data[end + 1] === 45;
    if (closing)
        end += 2;
    if (matches(data, newline, end))
        return { end: end + newline.length, closing };
    if (closing && end === data.length)
        return { end, closing };
    return undefined;
};
const generateBoundary = () => {
    if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
        return crypto.randomUUID().replace(/-/g, "");
    }
    let hex = "";
    for (let i = 0; i < 32; i++)
        hex += Math.floor(Math.random() * 16).toString(16);
    return hex;
};
const containsBoundary = (parts, boundary) => {
    const marker = encoder$1.encode(`--${boundary}`);
    const newline = encoder$1.encode(CRLF);
    return parts.some((part) => {
        // Appended framing can complete a delimiter at the end of a payload.
        const data = part.body;
        for (let i = data.indexOf(marker[0]); i >= 0; i = data.indexOf(marker[0], i + 1)) {
            if ((i === 0 || matches(data, newline, i - newline.length)) &&
                (delimiterAt(data, marker, newline, i) ||
                    (i + marker.length === data.length && matches(data, marker, i))))
                return true;
        }
        return false;
    });
};
const validateHeaderValues = (part) => {
    if (!/^[\x21-\x7e]+$/.test(part.name) || /["\\]/.test(part.name)) {
        throw new MultipartCodecError("invalid part name");
    }
    if (!/^[\x20-\x7e]+$/.test(part.contentType) || part.contentType.trim() !== part.contentType) {
        throw new MultipartCodecError("invalid Content-Type");
    }
};
/** Encode once, choosing a boundary that cannot frame a body. @since 1.5.0 @category operations */
const encodeMultipart = (parts) => {
    for (const part of parts)
        validateHeaderValues(part);
    let boundary = generateBoundary();
    while (containsBoundary(parts, boundary))
        boundary = generateBoundary();
    const chunks = [];
    for (const part of parts) {
        chunks.push(encoder$1.encode(`--${boundary}${CRLF}Content-Type: ${part.contentType}${CRLF}Content-Disposition: attachment; name="${part.name}"${CRLF}${CRLF}`));
        chunks.push(part.body, encoder$1.encode(CRLF));
    }
    chunks.push(encoder$1.encode(`--${boundary}--${CRLF}`));
    return { data: concat(chunks), boundary };
};
/** Decode complete delimiter lines, never trimming payload bytes. @since 1.5.0 @category operations */
const decodeMultipart = (data, boundary) => {
    if (!validBoundary(boundary))
        throw new MultipartCodecError("invalid boundary");
    const marker = encoder$1.encode(`--${boundary}`);
    let start = 0;
    if (matches(data, encoder$1.encode(CRLF), 0))
        start = 2;
    else if (data[0] === 10)
        start = 1;
    if (!matches(data, marker, start))
        throw new MultipartCodecError("Multipart body does not start with boundary");
    const firstSuffix = start + marker.length;
    const suffix = data[firstSuffix] === 45 && data[firstSuffix + 1] === 45 ? firstSuffix + 2 : firstSuffix;
    const newline = matches(data, encoder$1.encode(CRLF), suffix) || (suffix === data.length && start === 2)
        ? encoder$1.encode(CRLF)
        : encoder$1.encode("\n");
    if (start !== 0 && start !== newline.length)
        throw new MultipartCodecError("inconsistent leading newline");
    let delimiter = delimiterAt(data, marker, newline, start);
    if (!delimiter)
        throw new MultipartCodecError("invalid opening boundary");
    const parts = [];
    const names = new Set();
    while (!delimiter.closing) {
        let pos = delimiter.end;
        let name;
        let contentType;
        // Headers have their own textual newline rules; body framing uses the first delimiter's newline.
        while (true) {
            const end = data.indexOf(10, pos);
            if (end < 0)
                throw new MultipartCodecError("could not find end of headers in part");
            const lineEnd = data[end - 1] === 13 ? end - 1 : end;
            const bytes = data.subarray(pos, lineEnd);
            if (bytes.some((b) => b < 32 || b > 126))
                throw new MultipartCodecError("invalid ASCII header");
            const line = decoder.decode(bytes);
            pos = end + 1;
            if (line === "")
                break;
            if (!/^[\x20-\x7e]+$/.test(line) || /^\s/.test(line))
                throw new MultipartCodecError("invalid header");
            const colon = line.indexOf(":");
            if (colon < 0)
                throw new MultipartCodecError("invalid header");
            const key = line.slice(0, colon).toLowerCase();
            const value = line.slice(colon + 1).trim();
            if (key === "content-type") {
                if (contentType !== undefined)
                    throw new MultipartCodecError("duplicate Content-Type");
                contentType = value;
            }
            else if (key === "content-disposition") {
                if (name !== undefined)
                    throw new MultipartCodecError("duplicate Content-Disposition");
                const match = /^attachment;\s*name="([^"\\]+)"$/i.exec(value);
                if (!match)
                    throw new MultipartCodecError("part missing or invalid name in Content-Disposition");
                name = match[1];
            }
            else
                throw new MultipartCodecError("unsupported header");
        }
        if (name === undefined || contentType === undefined)
            throw new MultipartCodecError("part missing name or Content-Type");
        if (names.has(name))
            throw new MultipartCodecError(`Duplicate multipart part name: ${name}`);
        names.add(name);
        let next = undefined;
        let bodyEnd = data.indexOf(newline[0], pos);
        for (; bodyEnd >= 0; bodyEnd = data.indexOf(newline[0], bodyEnd + 1)) {
            if (matches(data, newline, bodyEnd)) {
                next = delimiterAt(data, marker, newline, bodyEnd + newline.length);
                if (next)
                    break;
            }
        }
        if (!next)
            throw new MultipartCodecError("missing closing boundary");
        const part = { name, contentType, body: data.slice(pos, bodyEnd) };
        validateHeaderValues(part);
        parts.push(part);
        delimiter = next;
    }
    if (delimiter.end !== data.length)
        throw new MultipartCodecError("data after closing boundary");
    return parts;
};
/** Extract a single valid boundary parameter. @since 1.5.0 @category utils */
const extractBoundary = (mimeType) => {
    const sections = mimeType.split(";");
    if (sections.shift()?.trim().toLowerCase() !== "multipart/mixed")
        return null;
    if (sections.length !== 1)
        return null;
    const match = /^\s*boundary\s*=\s*(?:"([^"\\]*)"|([^\s"=]+))\s*$/i.exec(sections[0]);
    if (!match)
        return null;
    if (match[2] !== undefined && !/^[A-Za-z0-9'+_.-]+$/.test(match[2]))
        return null;
    const boundary = match[1] ?? match[2];
    return validBoundary(boundary) ? boundary : null;
};

/**
 * @internal
 * @since 1.5.0
 */
/** A typed failure for envelope encode/decode operations. */
class SnapshotEnvelopeError {
    reason;
    _tag = "SnapshotEnvelopeError";
    constructor(reason) {
        this.reason = reason;
    }
}
/** A typed failure raised when a snapshot mime type is not supported. */
class UnsupportedSnapshotFormatError {
    mimeType;
    _tag = "UnsupportedSnapshotFormatError";
    constructor(mimeType) {
        this.mimeType = mimeType;
    }
}
const JSON_MIME = "application/json";
const BINARY_MIME = "application/octet-stream";
const MULTIPART_MIME_PREFIX = "multipart/mixed";
const SQLITE_PART_MIME = "application/x-sqlite3";
const STATE_PART_NAME = "state";
const DB_PART_PREFIX = "db:";
const USER_PART_PREFIX = "part:";
/** Validate logical user names before prefixing or inserting into maps. */
const validatePartName = (name) => {
    if (!/^[A-Za-z0-9_][A-Za-z0-9_.-]*$/.test(name)) {
        throw new SnapshotEnvelopeError(`invalid snapshot part name '${name}'`);
    }
};
/** Bare ASCII MIME type/subtype; parameters have no user-part semantics. */
const normalizeSnapshotContentType = (contentType) => {
    if (!/^[A-Za-z0-9!#$%&'*+.^_`|~-]+\/[A-Za-z0-9!#$%&'*+.^_`|~-]+$/.test(contentType)) {
        throw new SnapshotEnvelopeError(`invalid snapshot part Content-Type '${contentType}'`);
    }
    return contentType.toLowerCase();
};
// ---------------------------------------------------------------------------
// Schemas — the JSON-safe wire shapes
// ---------------------------------------------------------------------------
/**
 * Each OIDC string-typed sub-claim is encoded as `T | null` on the
 * wire (we explicitly emit `null` for absent fields), and tolerated
 * as missing on decode (so payloads produced by other SDKs that
 * simply omit absent claims still parse).
 */
const NullishString = Schema.optional(Schema.NullOr(Schema.String));
const NullishBoolean = Schema.optional(Schema.NullOr(Schema.Boolean));
/**
 * Schema for the JSON-safe `Principal` shape (UUIDs as strings).
 * Mirrors the runtime {@link AgentCommon.Principal} ADT but with all
 * `bigint` UUID payloads stringified so the value survives
 * `JSON.stringify` / `JSON.parse`.
 */
const SerializedPrincipal = Schema.Union([
    Schema.Struct({ tag: Schema.Literal("anonymous") }),
    Schema.Struct({
        tag: Schema.Literal("agent"),
        val: Schema.Struct({
            componentId: Schema.String,
            agentId: Schema.String,
        }),
    }),
    Schema.Struct({
        tag: Schema.Literal("golem-user"),
        val: Schema.Struct({ accountId: Schema.String }),
    }),
    Schema.Struct({
        tag: Schema.Literal("oidc"),
        val: Schema.Struct({
            sub: Schema.String,
            issuer: Schema.String,
            email: NullishString,
            name: NullishString,
            emailVerified: NullishBoolean,
            givenName: NullishString,
            familyName: NullishString,
            picture: NullishString,
            preferredUsername: NullishString,
            claims: Schema.String,
        }),
    }),
]);
/**
 * Schema for the full envelope: `{ version: 1, principal, state }`.
 * The `state` slot is intentionally `Schema.Unknown` — the user's own
 * Schema has already encoded it to a JSON value at the moment we
 * build the envelope, and re-validation happens against the user's
 * Schema after decode.
 */
const Envelope = Schema.Struct({
    version: Schema.Literal(1),
    principal: SerializedPrincipal,
    state: Schema.Unknown,
    fileDatabases: Schema.optional(Schema.Record(Schema.String, Schema.String)),
});
const EnvelopeFromString = Schema.fromJsonString(Envelope);
const PrincipalFromString = Schema.fromJsonString(SerializedPrincipal);
const encodeEnvelope = Schema.encodeUnknownSync(EnvelopeFromString);
const decodeEnvelopeFromString = Schema.decodeUnknownSync(EnvelopeFromString);
const encodePrincipal = Schema.encodeUnknownSync(PrincipalFromString);
const decodePrincipalFromString = Schema.decodeUnknownSync(PrincipalFromString);
const encoder = new TextEncoder();
const strictDecoder = strictTextDecoder();
const decodeUtf8 = (bytes, ctx) => {
    try {
        return strictDecoder.decode(bytes);
    }
    catch (err) {
        throw new SnapshotEnvelopeError(`${ctx}: payload is not valid UTF-8: ${String(err)}`);
    }
};
// ---------------------------------------------------------------------------
// Principal: runtime ↔ wire conversion
// ---------------------------------------------------------------------------
/**
 * Convert a runtime `Principal` (with bigint UUIDs) into a JSON-safe
 * envelope shape (with stringified UUIDs).
 */
const serializePrincipal = (p) => {
    switch (p.tag) {
        case "anonymous":
            return { tag: "anonymous" };
        case "agent":
            return {
                tag: "agent",
                val: {
                    componentId: CoreTypes.uuidToString(p.val.agentId.componentId.uuid),
                    agentId: p.val.agentId.agentId,
                },
            };
        case "golem-user":
            return {
                tag: "golem-user",
                val: { accountId: CoreTypes.uuidToString(p.val.accountId.uuid) },
            };
        case "oidc":
            return {
                tag: "oidc",
                val: {
                    sub: p.val.sub,
                    issuer: p.val.issuer,
                    email: p.val.email ?? null,
                    name: p.val.name ?? null,
                    emailVerified: p.val.emailVerified ?? null,
                    givenName: p.val.givenName ?? null,
                    familyName: p.val.familyName ?? null,
                    picture: p.val.picture ?? null,
                    preferredUsername: p.val.preferredUsername ?? null,
                    claims: p.val.claims,
                },
            };
    }
};
/**
 * Inverse of {@link serializePrincipal}: take the JSON-safe envelope
 * shape and rebuild the runtime `Principal` (parsing UUID strings back
 * into `{ highBits, lowBits }`).
 */
const deserializePrincipal = (p) => {
    switch (p.tag) {
        case "anonymous":
            return { tag: "anonymous" };
        case "agent":
            return {
                tag: "agent",
                val: {
                    agentId: {
                        componentId: { uuid: CoreTypes.parseUuid(p.val.componentId) },
                        agentId: p.val.agentId,
                    },
                },
            };
        case "golem-user":
            return {
                tag: "golem-user",
                val: { accountId: { uuid: CoreTypes.parseUuid(p.val.accountId) } },
            };
        case "oidc":
            return {
                tag: "oidc",
                val: {
                    sub: p.val.sub,
                    issuer: p.val.issuer,
                    email: p.val.email ?? undefined,
                    name: p.val.name ?? undefined,
                    emailVerified: p.val.emailVerified ?? undefined,
                    givenName: p.val.givenName ?? undefined,
                    familyName: p.val.familyName ?? undefined,
                    picture: p.val.picture ?? undefined,
                    preferredUsername: p.val.preferredUsername ?? undefined,
                    claims: p.val.claims,
                },
            };
    }
};
// ---------------------------------------------------------------------------
// Encode
// ---------------------------------------------------------------------------
/**
 * Encode an `application/json` envelope. `state` is an arbitrary JSON
 * value (already produced by the user's Schema-driven encode step).
 * Accepts a runtime `Principal` (with bigint UUIDs) and serializes it
 * via {@link serializePrincipal} before writing.
 */
const encodeJsonEnvelope = (principal, state) => {
    const json = encodeEnvelope({
        version: 1,
        principal: serializePrincipal(principal),
        state,
    });
    return {
        payload: encoder.encode(json),
        mimeType: JSON_MIME,
    };
};
/**
 * Encode an `application/octet-stream` envelope (binary v2). The user's
 * raw `payload` bytes are appended verbatim after the principal header.
 * Accepts a runtime `Principal` (with bigint UUIDs) and serializes it
 * via {@link serializePrincipal} before writing.
 */
const encodeBinaryEnvelope = (principal, userPayload) => {
    const principalJson = encodePrincipal(serializePrincipal(principal));
    const principalBytes = encoder.encode(principalJson);
    const total = 1 + 4 + principalBytes.length + userPayload.length;
    const out = new Uint8Array(total);
    const view = new DataView(out.buffer);
    view.setUint8(0, 2);
    view.setUint32(1, principalBytes.length, false); // big-endian
    out.set(principalBytes, 5);
    out.set(userPayload, 5 + principalBytes.length);
    return { payload: out, mimeType: BINARY_MIME };
};
/**
 * Encode a SQLite-aware `multipart/mixed` envelope. The `state` part
 * carries `{ version: 1, principal, state, fileDatabases? }` as JSON; each entry in
 * `databases` becomes a `db:<name>` part with `application/x-sqlite3`
 * content-type. File-backed database contents belong to filesystem snapshots;
 * only their logical names and locations belong in `fileDatabases`.
 */
const encodeMultipartJsonEnvelope = (principal, state, databases, userParts = new Map(), fileDatabases = databases.length > 0
    ? {}
    : undefined) => {
    if (state === undefined)
        throw new SnapshotEnvelopeError("multipart state must be JSON; use null for binary-only snapshots");
    const stateJson = encodeEnvelope({
        version: 1,
        principal: serializePrincipal(principal),
        state,
        ...(fileDatabases === undefined ? {} : { fileDatabases }),
    });
    const stateBody = encoder.encode(stateJson);
    const parts = [
        { name: STATE_PART_NAME, contentType: JSON_MIME, body: stateBody },
        ...databases.map((db) => ({
            name: `${DB_PART_PREFIX}${db.name}`,
            contentType: SQLITE_PART_MIME,
            body: db.bytes,
        })),
        ...Array.from(userParts, ([name, part]) => {
            validatePartName(name);
            if (!(part.bytes instanceof Uint8Array))
                throw new SnapshotEnvelopeError(`part '${name}' bytes must be Uint8Array`);
            return {
                name: `${USER_PART_PREFIX}${name}`,
                contentType: normalizeSnapshotContentType(part.contentType),
                body: part.bytes,
            };
        }),
    ];
    const { data, boundary } = encodeMultipart(parts);
    return { payload: data, mimeType: `${MULTIPART_MIME_PREFIX}; boundary=${boundary}` };
};
/**
 * Decode a {@link ApiHost.Snapshot}. `fallbackPrincipal` is used when
 * the snapshot is the legacy binary v1 format which carries no
 * principal in its payload.
 */
const decodeEnvelope = (snapshot, fallbackPrincipal) => {
    const mime = snapshot.mimeType;
    if (mime.split(";", 1)[0]?.trim().toLowerCase() === MULTIPART_MIME_PREFIX) {
        return decodeMultipartEnvelope(snapshot.payload, mime);
    }
    if (mime === JSON_MIME) {
        return decodeJsonEnvelope(snapshot.payload);
    }
    if (mime === BINARY_MIME) {
        return decodeBinaryEnvelope(snapshot.payload);
    }
    throw new UnsupportedSnapshotFormatError(mime);
};
const parseEnvelopeJson = (text, ctx, strict = false) => {
    try {
        if (strict) {
            // JSON.parse validates syntax but discards duplicate keys. Inspect the
            // valid token stream before schema decoding so metadata cannot be replaced.
            const value = JSON.parse(text);
            if (value === null || typeof value !== "object" || !Object.hasOwn(value, "state")) {
                throw new Error("multipart envelope missing state");
            }
            const tokens = text.match(/"(?:[^"\\]|\\.)*"|[{}[\],:]|[^{}[\],:\s]+/g);
            const objects = [];
            for (let i = 0; i < tokens.length; i++) {
                const token = tokens[i];
                if (token === "{")
                    objects.push(new Set());
                else if (token === "[")
                    objects.push(undefined);
                else if (token === "}" || token === "]")
                    objects.pop();
                else if (token.startsWith('"') && tokens[i + 1] === ":") {
                    const key = JSON.parse(token);
                    const keys = objects[objects.length - 1];
                    if (keys.has(key))
                        throw new Error(`duplicate JSON key '${key}'`);
                    keys.add(key);
                    if (objects.length === 1 && key === "version" && tokens[i + 2] !== "1") {
                        throw new Error("multipart envelope version must be integer 1");
                    }
                }
            }
        }
        return decodeEnvelopeFromString(text);
    }
    catch (err) {
        throw new SnapshotEnvelopeError(`${ctx}: ${String(err.message ?? err)}`);
    }
};
const decodeJsonEnvelope = (payload) => {
    const text = decodeUtf8(payload, "json envelope");
    const env = parseEnvelopeJson(text, "json envelope");
    return {
        kind: "json",
        principal: deserializePrincipal(env.principal),
        state: env.state,
    };
};
const decodeMultipartEnvelope = (payload, mime) => {
    const boundary = extractBoundary(mime);
    if (boundary === null) {
        throw new UnsupportedSnapshotFormatError(mime);
    }
    let parts;
    try {
        parts = decodeMultipart(payload, boundary);
    }
    catch (err) {
        if (err instanceof MultipartCodecError) {
            throw new SnapshotEnvelopeError(`multipart envelope: ${err.reason}`);
        }
        throw err;
    }
    const statePart = parts.find((p) => p.name === STATE_PART_NAME);
    if (!statePart) {
        throw new SnapshotEnvelopeError(`multipart envelope: missing 'state' part`);
    }
    if (statePart.contentType !== JSON_MIME) {
        throw new SnapshotEnvelopeError(`multipart envelope: 'state' part has Content-Type '${statePart.contentType}' (expected '${JSON_MIME}')`);
    }
    const stateText = decodeUtf8(statePart.body, "multipart envelope: 'state' part");
    // Enforce strict UTF-8 even when the guest runtime lacks fatal TextDecoder.
    const reencoded = encoder.encode(stateText);
    if (reencoded.length !== statePart.body.length ||
        reencoded.some((b, i) => b !== statePart.body[i])) {
        throw new SnapshotEnvelopeError("multipart envelope state is not canonical UTF-8");
    }
    const env = parseEnvelopeJson(stateText, "multipart envelope: 'state' part", true);
    const principal = deserializePrincipal(env.principal);
    const databases = [];
    const userParts = new Map();
    for (const part of parts) {
        if (part.name === STATE_PART_NAME)
            continue;
        if (part.name.startsWith(USER_PART_PREFIX)) {
            const name = part.name.slice(USER_PART_PREFIX.length);
            validatePartName(name);
            userParts.set(name, {
                bytes: part.body,
                contentType: normalizeSnapshotContentType(part.contentType),
            });
            continue;
        }
        if (!part.name.startsWith(DB_PART_PREFIX)) {
            throw new SnapshotEnvelopeError(`multipart envelope: unrecognised part name '${part.name}' (expected 'state', 'part:<name>' or 'db:<name>')`);
        }
        if (part.contentType !== SQLITE_PART_MIME) {
            throw new SnapshotEnvelopeError(`multipart envelope: '${part.name}' part has Content-Type '${part.contentType}' (expected '${SQLITE_PART_MIME}')`);
        }
        const dbName = part.name.slice(DB_PART_PREFIX.length);
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(dbName))
            throw new SnapshotEnvelopeError(`invalid database name '${dbName}'`);
        databases.push({ name: dbName, bytes: part.body });
    }
    if (env.fileDatabases !== undefined) {
        for (const [name, location] of Object.entries(env.fileDatabases)) {
            if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name))
                throw new SnapshotEnvelopeError(`invalid database name '${name}'`);
            if (location.length === 0 || location === ":memory:" || location.includes("\0"))
                throw new SnapshotEnvelopeError(`invalid file database location for '${name}'`);
            if (databases.some((part) => part.name === name))
                throw new SnapshotEnvelopeError(`database '${name}' has both an image and a file location`);
        }
    }
    return {
        kind: "multipart",
        principal,
        state: env.state,
        databases,
        fileDatabases: env.fileDatabases,
        parts: userParts,
    };
};
const decodeBinaryEnvelope = (payload, _fallbackPrincipal) => {
    if (payload.length < 1) {
        throw new SnapshotEnvelopeError(`binary envelope: payload is empty`);
    }
    const version = payload[0];
    if (version === 2) {
        if (payload.length < 5) {
            throw new SnapshotEnvelopeError(`binary envelope (v2): truncated header`);
        }
        const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
        const principalLen = view.getUint32(1, false);
        if (payload.length < 5 + principalLen) {
            throw new SnapshotEnvelopeError(`binary envelope (v2): declared principal length ${principalLen} exceeds payload size ${payload.length - 5}`);
        }
        const principalBytes = payload.slice(5, 5 + principalLen);
        const principalText = decodeUtf8(principalBytes, "binary envelope (v2): principal segment");
        let serialized;
        try {
            serialized = decodePrincipalFromString(principalText);
        }
        catch (err) {
            throw new SnapshotEnvelopeError(`binary envelope (v2): principal segment is not a valid principal: ${String(err.message ?? err)}`);
        }
        return {
            kind: "binary",
            principal: deserializePrincipal(serialized),
            userPayload: payload.slice(5 + principalLen),
        };
    }
    throw new SnapshotEnvelopeError(`binary envelope: unsupported version byte ${version}`);
};

/**
 * SQLite adapter for `effect-golem` agents — implements the official
 * `effect/sql/SqlClient` interface on top of Node's built-in
 * `node:sqlite` (`DatabaseSync`/`StatementSync`) so it works both on
 * Node 22+ (for unit tests and local development) and inside Golem's
 * `wasm-rquickjs` runtime.
 *
 * Why not `@effect/sql-sqlite-node`? That package depends on
 * `better-sqlite3` (a native N-API addon) which cannot run inside
 * `wasm-rquickjs`. This module mirrors the structure of
 * `@effect/sql-sqlite-node`'s adapter (so users can use `SqlSchema`,
 * `SqlResolver`, `Migrator` against it) but delegates to `node:sqlite`
 * primitives and the wasm-rquickjs host extensions for
 * `serializeDatabaseSync` / `restoreDatabaseSync` /
 * `isAutocommitDatabaseSync`.
 *
 * Skipped on purpose:
 * - `executeStream` (no native cursor on `StatementSync`; the
 *   connection returns `Stream.die("not implemented")` to match how
 *   `@effect/sql-sqlite-node` handles the same gap).
 * - `loadExtension` and custom user functions / aggregates (not safely
 *   exposed by `node:sqlite`).
 *
 * @since 1.5.0
 */
// ---------------------------------------------------------------------------
// TypeId / cross-bundle identity
// ---------------------------------------------------------------------------
/**
 * Unique symbol stamped on every {@link SqliteClient} instance so
 * `attachDatabase` (and other consumers) can reliably distinguish a
 * client from a raw `DatabaseSync`. The symbol is keyed via
 * `Symbol.for(...)` so multiple module copies (e.g. one bundled into
 * `effect-golem`'s main bundle for `src/snapshot.ts`'s relative
 * import of `./Sqlite.js`, and one in the standalone
 * `@golemcloud/effect-golem/sqlite` sub-import) still agree on the same key.
 *
 * @since 1.5.0
 * @category symbols
 */
const SqliteClientTypeId = Symbol.for("effect-golem/SqliteClient");
/**
 * Globally-keyed symbol used to stash the underlying `DatabaseSync`
 * directly on the SqliteClient object so {@link __getUnderlyingDatabase}
 * works regardless of which bundle the client originated in.
 */
const UnderlyingDbSymbol = Symbol.for("effect-golem/SqliteClient/__db");
// ---------------------------------------------------------------------------
// Effect Context tag
// ---------------------------------------------------------------------------
/**
 * Context tag for resolving an {@link SqliteClient} from the
 * environment. Both this tag and the upstream `Client.SqlClient` tag
 * are populated by {@link SqliteClient.layer}.
 *
 * @since 1.5.0
 * @category host services
 */
class SqliteClientService extends Context.Service()("effect-golem/SqliteClient") {
}
// ---------------------------------------------------------------------------
// Helpers consumed by `src/snapshot.ts`
// ---------------------------------------------------------------------------
/**
 * Probe an arbitrary value for the SqliteClient brand.
 *
 * @since 1.5.0
 * @category guards
 */
const isSqliteClient = (v) => {
    if (v === null)
        return false;
    // The SqliteClient is callable (it extends `effect/sql`'s
    // `Constructor`), so it shows up as `typeof === "function"` — accept
    // both function and object targets here.
    const kind = typeof v;
    if (kind !== "object" && kind !== "function")
        return false;
    return (v[SqliteClientTypeId] === SqliteClientTypeId);
};
/**
 * Internal: extract the underlying `DatabaseSync` from a
 * {@link SqliteClient}. Used by `snapshot.ts` to capture the handle
 * for `serializeDatabaseSync` / `restoreDatabaseSync`. NOT part of the
 * public surface.
 *
 * @internal
 * @since 1.5.0
 */
const __getUnderlyingDatabase = (client) => {
    const db = client[UnderlyingDbSymbol];
    if (db === undefined) {
        throw new Error("SqliteClient was not built by effect-golem; cannot extract DatabaseSync");
    }
    return db;
};

const isFiniteInteger = (n) => Number.isInteger(n) && Number.isFinite(n);
const policyToWit = (p, ctxLabel) => {
    switch (p._tag) {
        case "Default":
            return Effect.succeed({ tag: "default" });
        case "Periodic": {
            const d = Duration.fromInputUnsafe(p.duration);
            const nanos = Duration.toNanosUnsafe(d);
            // wasi:clocks/monotonic-clock.duration = u64 nanoseconds.
            return Effect.succeed({ tag: "periodic", val: nanos < 0n ? 0n : nanos });
        }
        case "EveryN": {
            if (!isFiniteInteger(p.n) || p.n < 1 || p.n > 0xffff) {
                return Effect.fail(new InvalidSnapshotError(`${ctxLabel}: snapshot policy 'everyN' must be a positive integer in 1..=65535 (got ${p.n})`));
            }
            return Effect.succeed({ tag: "every-n-invocation", val: p.n });
        }
    }
};
// ---------------------------------------------------------------------------
// Typed errors
// ---------------------------------------------------------------------------
/**
 * Surfaced from `registerAgent` for malformed
 * `Snapshot.define`/`Snapshot.custom` shapes.
 *
 * @since 1.5.0
 * @category errors
 */
class InvalidSnapshotError {
    reason;
    _tag = "InvalidSnapshotError";
    message;
    constructor(reason) {
        this.reason = reason;
        this.message = `InvalidSnapshotError: ${reason}`;
    }
}
/**
 * Raised when a snapshot strategy exposes a database name that was not
 * declared in `Snapshot.define({ databases: [...] })`.
 *
 * @since 1.5.0
 * @category errors
 */
class SnapshotDatabaseNotDeclaredError {
    agentName;
    databaseName;
    _tag = "SnapshotDatabaseNotDeclaredError";
    message;
    constructor(agentName, databaseName) {
        this.agentName = agentName;
        this.databaseName = databaseName;
        this.message = `SnapshotDatabaseNotDeclaredError: agent '${agentName}' exposed database '${databaseName}' but '${databaseName}' is not listed in 'Snapshot.define({ databases: [...] })'`;
    }
}
/**
 * Raised when, at save or load time, a declared database name has no
 * corresponding database exposed by the state strategy (`phase: "save"` or
 * `phase: "load-attach"`) or no corresponding
 * image or file location in the loaded envelope (`phase: "load-envelope"`).
 *
 * @since 1.5.0
 * @category errors
 */
class SnapshotDatabaseMissingPartError {
    agentName;
    databaseName;
    phase;
    _tag = "SnapshotDatabaseMissingPartError";
    message;
    constructor(agentName, databaseName, phase) {
        this.agentName = agentName;
        this.databaseName = databaseName;
        this.phase = phase;
        switch (phase) {
            case "save":
                this.message = `SnapshotDatabaseMissingPartError: agent '${agentName}' save: declared database '${databaseName}' but the snapshot strategy did not expose it`;
                break;
            case "load-attach":
                this.message = `SnapshotDatabaseMissingPartError: agent '${agentName}' load: declared database '${databaseName}' but the restored state's snapshot strategy did not expose it`;
                break;
            case "load-envelope":
                this.message = `SnapshotDatabaseMissingPartError: agent '${agentName}' load: snapshot envelope is missing the image or file location for database '${databaseName}'`;
                break;
        }
    }
}
/**
 * Raised at load time when a snapshot envelope contains an image or file location
 * whose name is not in the agent's declared `databases`
 * tuple.
 *
 * @since 1.5.0
 * @category errors
 */
class SnapshotDatabaseUnknownPartError {
    agentName;
    databaseName;
    _tag = "SnapshotDatabaseUnknownPartError";
    message;
    constructor(agentName, databaseName) {
        this.agentName = agentName;
        this.databaseName = databaseName;
        this.message = `SnapshotDatabaseUnknownPartError: agent '${agentName}' load: snapshot envelope carries database '${databaseName}' but the agent did not declare a database with that name`;
    }
}
/**
 * Raised when a declared database has an open transaction
 * (`isAutocommitDatabaseSync` returns false).
 *
 * @since 1.5.0
 * @category errors
 */
class SnapshotDatabaseNotInAutocommitError {
    agentName;
    databaseName;
    _tag = "SnapshotDatabaseNotInAutocommitError";
    message;
    constructor(agentName, databaseName) {
        this.agentName = agentName;
        this.databaseName = databaseName;
        this.message = `SnapshotDatabaseNotInAutocommitError: agent '${agentName}' database '${databaseName}' has an open transaction; snapshot connections must be in autocommit mode`;
    }
}
/**
 * Raised when a declared database has ATTACHed schemas
 * beyond the default `main`/`temp` (PRAGMA database_list).
 *
 * @since 1.5.0
 * @category errors
 */
class SnapshotDatabaseHasAttachmentsError {
    agentName;
    databaseName;
    extraSchemas;
    _tag = "SnapshotDatabaseHasAttachmentsError";
    message;
    constructor(agentName, databaseName, extraSchemas) {
        this.agentName = agentName;
        this.databaseName = databaseName;
        this.extraSchemas = extraSchemas;
        this.message = `SnapshotDatabaseHasAttachmentsError: agent '${agentName}' database '${databaseName}' has ATTACHed schemas (${extraSchemas.join(", ")}) — only main/temp may be present at snapshot time`;
    }
}
/** Pattern that database names must match. */
const DB_NAME_RE = /^[a-zA-Z_][a-zA-Z0-9_]*$/;
/**
 * Compile a {@link SnapshotDef} produced by `Snapshot.define` /
 * `Snapshot.custom`. Walks the schema (auto path), validates the
 * policy, and produces the WIT-side `snapshotting-config`.
 *
 * @since 1.5.0
 * @category metadata
 */
const compileSnapshot = (agentName, def) => Effect.gen(function* () {
    const witConfig = yield* policyToWit(def.policy, `agent '${agentName}' snapshot`);
    if (def._tag === "AutoSnapshotDef" || def._tag === "MultipartSnapshotDef") {
        const rawDbs = def.databases ?? [];
        const seenNames = new Set();
        for (const name of rawDbs) {
            if (!DB_NAME_RE.test(name)) {
                return yield* Effect.fail(new InvalidSnapshotError(`agent '${agentName}' snapshot: database name '${name}' is invalid; expected /^[a-zA-Z_][a-zA-Z0-9_]*$/`));
            }
            if (seenNames.has(name)) {
                return yield* Effect.fail(new InvalidSnapshotError(`agent '${agentName}' snapshot: database name '${name}' appears more than once in 'databases'`));
            }
            seenNames.add(name);
        }
        return {
            kind: def._tag === "AutoSnapshotDef" ? "auto" : "multipart",
            policy: def.policy,
            witConfig,
            schema: def.schema,
            declaredDatabases: [...rawDbs],
        };
    }
    return { kind: "custom", policy: def.policy, witConfig };
});
/**
 * Resolve the SQLite handles exposed by an initialized or restored state.
 * Unknown names are rejected immediately; declared names omitted here remain
 * subject to the save/load missing-part checks.
 *
 * @since 1.6.0
 * @category constructors
 */
const createSnapshot = (agentName, compiled, databases = {}) => {
    if (compiled.kind === "custom") {
        const unknownName = Object.keys(databases)[0];
        if (unknownName !== undefined) {
            throw new SnapshotDatabaseNotDeclaredError(agentName, unknownName);
        }
        return { kind: "custom" };
    }
    const declared = new Set(compiled.declaredDatabases);
    const resolved = new Map();
    for (const [name, database] of Object.entries(databases)) {
        if (!declared.has(name)) {
            throw new SnapshotDatabaseNotDeclaredError(agentName, name);
        }
        resolved.set(name, isSqliteClient(database) ? __getUnderlyingDatabase(database) : database);
    }
    return {
        kind: compiled.kind,
        schema: compiled.schema,
        declaredDatabases: compiled.declaredDatabases,
        databases: resolved,
    };
};

const marker = "$golemSnapshot";
const decimal = /^(?:0|-?[1-9][0-9]*)$/;
/** Encode only schema-encoded state; envelope metadata and binary parts are not state. */
const encodeSnapshotState = (value) => {
    if (Predicate.isBigInt(value))
        return { [marker]: ["bigint", value.toString()] };
    if (Array.isArray(value))
        return value.map(encodeSnapshotState);
    if (Predicate.isObject(value)) {
        const prototype = Object.getPrototypeOf(value);
        if (prototype !== Object.prototype && prototype !== null)
            return value;
        const entries = Object.entries(value).map(([key, item]) => [key, encodeSnapshotState(item)]);
        return Object.hasOwn(value, marker)
            ? { [marker]: ["object", entries] }
            : Object.fromEntries(entries);
    }
    return value;
};
const decodeSnapshotState = (value) => {
    if (Array.isArray(value))
        return value.map(decodeSnapshotState);
    if (!Predicate.isObject(value))
        return value;
    if (Object.hasOwn(value, marker) && Object.keys(value).length === 1) {
        const tag = value[marker];
        if (Array.isArray(tag) && tag.length === 2) {
            if (tag[0] === "bigint" &&
                Predicate.isString(tag[1]) &&
                decimal.exec(tag[1])?.[0] === tag[1]) {
                return BigInt(tag[1]);
            }
            if (tag[0] === "object" && Array.isArray(tag[1])) {
                const keys = new Set();
                return Object.fromEntries(tag[1].map((entry) => {
                    if (!Array.isArray(entry) ||
                        entry.length !== 2 ||
                        !Predicate.isString(entry[0]) ||
                        keys.has(entry[0])) {
                        throw new SnapshotEnvelopeError("invalid snapshot state object entry");
                    }
                    keys.add(entry[0]);
                    return [entry[0], decodeSnapshotState(entry[1])];
                }));
            }
        }
        throw new SnapshotEnvelopeError("invalid snapshot state tag");
    }
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, decodeSnapshotState(item)]));
};

// ---------------------------------------------------------------------------
// Level mapping
// ---------------------------------------------------------------------------
/**
 * Map an Effect `LogLevel` to a `wasi:logging/logging` level. Effect's
 * `All` is treated as `trace` (most permissive); `None` is treated as
 * `info` (it should never reach a logger after filtering, but mapping
 * defensively keeps the call total).
 *
 * @since 1.5.0
 * @category utils
 */
const wasiLevelOf = (level) => {
    switch (level) {
        case "All":
            return "trace";
        case "Trace":
            return "trace";
        case "Debug":
            return "debug";
        case "Info":
            return "info";
        case "Warn":
            return "warn";
        case "Error":
            return "error";
        case "Fatal":
            return "critical";
        case "None":
        default:
            return "info";
    }
};
// ---------------------------------------------------------------------------
// Safe stringification
// ---------------------------------------------------------------------------
/**
 * `JSON.stringify` that survives `bigint`, cycles, undefined, and
 * functions. Used to render log annotations / message payloads — must
 * never throw inside a logger callback.
 *
 * @since 1.5.0
 * @category utils
 */
const safeStringify = (value) => {
    if (value === undefined)
        return "undefined";
    if (value === null)
        return "null";
    if (typeof value === "string")
        return value;
    if (typeof value === "bigint")
        return `${value.toString()}n`;
    if (typeof value === "number" || typeof value === "boolean")
        return String(value);
    if (typeof value === "function")
        return `[function ${value.name || "anonymous"}]`;
    const seen = new WeakSet();
    try {
        return (JSON.stringify(value, (_k, v) => {
            if (typeof v === "bigint")
                return `${v.toString()}n`;
            if (typeof v === "function")
                return `[function ${v.name || "anonymous"}]`;
            if (v && typeof v === "object") {
                if (seen.has(v))
                    return "[Circular]";
                seen.add(v);
            }
            return v;
        }) ?? "undefined");
    }
    catch (e) {
        return `[unstringifiable: ${e instanceof Error ? e.message : String(e)}]`;
    }
};
// ---------------------------------------------------------------------------
// Logger formatter
// ---------------------------------------------------------------------------
const escapeValue = (value) => {
    if (value === "")
        return '""';
    if (/[\s"=]/.test(value)) {
        return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
    }
    return value;
};
const renderMessage = (message) => {
    if (Array.isArray(message)) {
        return message.map(safeStringify).join(" ");
    }
    return safeStringify(message);
};
const ZERO_TRACE_ID$1 = "00000000000000000000000000000000";
const ZERO_SPAN_ID$1 = "0000000000000000";
/**
 * Read the current host invocation context's trace + span ids via the
 * provided {@link TracingHost} service. All-zero ids are treated as
 * "no active host span" and elided from the rendered log line.
 */
const safeCurrentSpanIds = (tracing) => {
    try {
        const ctx = tracing.currentContext();
        const traceId = ctx.traceId();
        const spanId = ctx.spanId();
        if (traceId === ZERO_TRACE_ID$1 || spanId === ZERO_SPAN_ID$1)
            return {};
        return { traceId, spanId };
    }
    catch {
        return {};
    }
};
/**
 * Render the per-call log line. Best-effort and non-throwing — used
 * inside the synchronous logger callback.
 */
const formatLine = (level, fiberId, message, cause, annotations, spans, date, traceId, spanId) => {
    const parts = [];
    parts.push(`level=${level.toLowerCase()}`);
    parts.push(`fiber=${fiberId}`);
    parts.push(`ts=${date.toISOString()}`);
    if (spans.length > 0) {
        const labels = spans.map(([label]) => label).join(",");
        parts.push(`spans=${escapeValue(labels)}`);
    }
    if (traceId !== undefined && traceId.length > 0) {
        parts.push(`trace_id=${traceId}`);
    }
    if (spanId !== undefined && spanId.length > 0) {
        parts.push(`span_id=${spanId}`);
    }
    for (const [k, v] of Object.entries(annotations)) {
        parts.push(`${k}=${escapeValue(safeStringify(v))}`);
    }
    if (cause.reasons.length > 0) {
        parts.push(`cause=${escapeValue(Cause.pretty(cause))}`);
    }
    return `${parts.join(" ")} :: ${renderMessage(message)}`;
};
// ---------------------------------------------------------------------------
// Logger factory
// ---------------------------------------------------------------------------
/**
 * Build the Effect `Logger` that forwards every log call to the host's
 * `wasi:logging.log`. The logger closes over the {@link LoggingHost}
 * and {@link TracingHost} services captured at layer-construction
 * time, so the synchronous logger callback can read the current host
 * context without re-yielding from the service tags.
 *
 * Best-effort and non-throwing: a host failure or stringification
 * error never propagates back into user code.
 */
const makeGolemLogger = (logging, tracing) => Logger.make((options) => {
    try {
        const annotations = options.fiber.getRef(References.CurrentLogAnnotations);
        const spans = options.fiber.getRef(References.CurrentLogSpans);
        const ids = safeCurrentSpanIds(tracing);
        const fiberId = typeof options.fiber.id === "string" ? options.fiber.id : `#${String(options.fiber.id)}`;
        const line = formatLine(options.logLevel, fiberId, options.message, options.cause, annotations, spans, options.date, ids.traceId, ids.spanId);
        logging.log(wasiLevelOf(options.logLevel), "", line);
    }
    catch {
        // Telemetry must never break business logic.
    }
});
const golemLoggerEffect = Effect.gen(function* () {
    const logging = yield* LoggingHost;
    const tracing = yield* TracingHost;
    return makeGolemLogger(logging, tracing);
});
// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------
/**
 * Replace Effect's default logger set with the Golem host logger. This
 * is the production wiring: every `Effect.log*` call in user code lands
 * in `wasi:logging` and nothing else. Requires `LoggingHost` and
 * `TracingHost` (provided by `HostLive`).
 *
 * @since 1.5.0
 * @category layers
 */
const layer$1 = Logger.layer([
    golemLoggerEffect,
]);
/**
 * Add the Golem host logger alongside Effect's default loggers.
 * Convenient for dev / vitest where console output is also helpful.
 *
 * @since 1.5.0
 * @category layers
 */
Logger.layer([golemLoggerEffect], { mergeWithExisting: true });

/**
 * Fiber-local custom durability scopes.
 *
 * Every fiber carries the token of the custom invocation scope it runs in. A composed
 * `Tracer.context` hook installs a fiber's token on the first step of each run segment and
 * restores the caller's token when the segment ends (a step returns `Yield` or throws). A
 * segment can run synchronously inside another fiber's segment, so frames form a stack.
 * Host calls that start inside a segment are attributed to that fiber's scope; the runtime
 * carries the token across the native Promise jobs, timers and host bridges they create.
 */
/** The custom scope of the current fiber. Forks inherit it; `undefined` outside an entry point. */
const CurrentCustomScope = Context.Reference("effect-golem/CurrentCustomScope", { defaultValue: () => undefined });
// The run loop's segment-end marker, a registry symbol shared by every Effect copy.
const Yield = Symbol.for("effect/Effect/Yield");
const evaluate = "~effect/Effect/evaluate";
const scopeClient = Symbol.for("effect-golem/custom-scope-client");
const frames = [];
const isComposed = (tracer) => scopeClient in tracer;
// The parent of each scope that the SDK closed.
const closedScopes = new WeakMap();
// The nearest open ancestor of a token, as the runtime resolves it. Restoring a frame installs it,
// because the runtime reports every install of a closed token as escaped work.
const nearestOpen = (token) => {
    let open = token;
    let parent = closedScopes.get(open);
    while (parent !== undefined) {
        open = parent;
        parent = closedScopes.get(open);
    }
    return open;
};
const swapBack = (client, token) => {
    client.swap(nearestOpen(token));
};
const leave = (client, frame) => {
    frames.pop();
    if (frame.previous !== undefined)
        swapBack(client, frame.previous);
};
/** Wraps `inner` so that every fiber step runs in the fiber's custom scope. */
const composeTracer = (inner, client) => {
    if (isComposed(inner))
        return inner;
    const step = (primitive, fiber) => inner.context === undefined
        ? primitive[evaluate](fiber)
        : inner.context(primitive, fiber);
    const composed = {
        [scopeClient]: client,
        span: (options) => inner.span(options),
        context: (primitive, fiber) => {
            const token = fiber.getRef(CurrentCustomScope);
            let frame = frames[frames.length - 1];
            if (frame === undefined || frame.fiber !== fiber) {
                frame = { fiber, previous: token === undefined ? undefined : client.swap(token), token };
                frames.push(frame);
            }
            else if (frame.token !== token) {
                if (token !== undefined) {
                    const previous = client.swap(token);
                    frame.previous ??= previous;
                }
                else if (frame.previous !== undefined) {
                    swapBack(client, frame.previous);
                }
                frame.token = token;
            }
            let result;
            try {
                result = step(primitive, fiber);
            }
            catch (error) {
                leave(client, frame);
                throw error;
            }
            if (result === Yield)
                leave(client, frame);
            return result;
        },
    };
    return composed;
};
/**
 * Schedules fiber work as Promise jobs registered in the `root` scope. The default scheduler
 * yields through `setImmediate`, whose timer is a recorded root host call: a fiber resuming in
 * a custom scope would leave a root call that replay of a completed scope cannot reproduce.
 * Each fiber step installs its own scope, so the job's scope only avoids running in a closed one.
 */
const customScopeScheduler = (client, root) => new Scheduler.MixedScheduler("async", (task) => {
    let cancelled = false;
    client.run(root, () => Promise.resolve().then(() => {
        if (!cancelled)
            task();
    }));
    return () => {
        cancelled = true;
    };
});
/**
 * Runs a dispatcher entry with the scope hook and the runtime's current token as the root
 * scope of every fiber it starts.
 */
const withCustomScopes = (effect) => Effect.gen(function* () {
    const client = yield* CustomInvocationContextClient;
    const tracer = yield* Effect.tracer;
    const root = client.capture();
    return yield* effect.pipe(Effect.withTracer(composeTracer(tracer, client)), Effect.provideService(CurrentCustomScope, root), Effect.provideService(Scheduler.Scheduler, customScopeScheduler(client, root)));
});
class CustomScopeHookMissingError extends Error {
    _tag = "CustomScopeHookMissingError";
    constructor() {
        super("Durability.wrap needs the Golem tracer scope hook; compose a replacement Tracer with Tracing.compose");
    }
}
/**
 * The current fiber's scope, or its nearest open ancestor when the fiber outlived it, so that a new
 * operation registers with the parent the runtime enters it under. Dies when the fiber runs without
 * the scope hook.
 */
Effect.flatMap(Effect.tracer, (tracer) => Effect.withFiber((fiber) => {
    const token = fiber.getRef(CurrentCustomScope);
    return token !== undefined && isComposed(tracer)
        ? Effect.succeed(nearestOpen(token))
        : Effect.die(new CustomScopeHookMissingError());
}));

/**
 * Effect-idiomatic façade over `golem:api/context@1.5.0`.
 *
 * Provides:
 *
 * - {@link layer} — installs a Tracer that creates host spans through
 *   `golem:api/context.startSpan`, so every `Effect.withSpan` ends up
 *   in the Golem oplog / propagated trace context. This is the
 *   production wiring; the agent dispatcher applies it automatically.
 *   Requires {@link TracingHost} (provided by `HostLive`).
 * - {@link withInvocationParent} — captures the host's current
 *   invocation context and wraps a program with the matching Effect
 *   `ExternalSpan` parent, so user-side `Effect.withSpan` chains under
 *   the live invocation root. Effect-typed: requires {@link TracingHost}.
 * - {@link currentContext} / {@link traceContextHeaders} — Effect-typed
 *   wrappers around the host's snapshot accessors.
 * - {@link allowForwardingTraceContextHeaders} / {@link withForwardedHeaders}
 *   — toggle for outgoing-HTTP propagation, available imperatively or
 *   as a scoped helper.
 *
 * Limitations (intentional, see oracle review):
 *
 * - The host has no concept of span events; `Effect.log*` calls become
 *   `wasi:logging` lines (via the Logger layer) but are NOT replayed as
 *   host span events. Span-event state is kept locally on the
 *   {@link GolemSpan} so anything reading them in-process still works.
 * - The host has no concept of span links; addLinks is captured locally
 *   only.
 * - The host's `startSpan` always parents the new span under
 *   `currentContext()`. We accept this for sequential / structured
 *   nested spans (the typical Effect use case). When `Effect.withSpan`
 *   requests an explicit `parent` or `root: true` AND that hint
 *   conflicts with the host's stack, we fall back to a local
 *   `Tracer.NativeSpan` so the host stack is never corrupted.
 *
 * @since 1.5.0
 */
// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------
/**
 * Raised when one of the imperative `Tracing.*` host calls throws.
 *
 * @since 1.5.0
 * @category errors
 */
class TracingHostError {
    cause;
    _tag = "TracingHostError";
    message;
    constructor(cause) {
        this.cause = cause;
        this.message = `TracingHostError: ${cause instanceof Error ? cause.message : String(cause)}`;
    }
}
// ---------------------------------------------------------------------------
// Attribute coercion
// ---------------------------------------------------------------------------
/**
 * Coerce an arbitrary Effect span attribute into the host's
 * `{ tag: "string", val }` shape. The host only carries string
 * attributes today, so all primitive types collapse to `String(value)`,
 * `Date` becomes ISO-8601, and composites use {@link safeStringify}.
 */
const toAttributeValue = (value) => {
    if (typeof value === "string")
        return { tag: "string", val: value };
    if (typeof value === "number" || typeof value === "boolean") {
        return { tag: "string", val: String(value) };
    }
    if (typeof value === "bigint")
        return { tag: "string", val: `${value.toString()}n` };
    if (value === undefined)
        return { tag: "string", val: "undefined" };
    if (value === null)
        return { tag: "string", val: "null" };
    if (value instanceof Date)
        return { tag: "string", val: value.toISOString() };
    return { tag: "string", val: safeStringify(value) };
};
// ---------------------------------------------------------------------------
// GolemSpan — Effect Span backed by a host Span resource handle
// ---------------------------------------------------------------------------
const ZERO_TRACE_ID = "00000000000000000000000000000000";
const ZERO_SPAN_ID = "0000000000000000";
/**
 * Decide whether a given `Effect.withSpan` request can safely be
 * delegated to the host. The host always parents new spans under
 * `currentContext()`, so we can delegate when:
 *
 *  - there is no explicit Effect parent (the host's current span IS
 *    the right parent — typically the invocation root or an outer
 *    `GolemSpan` we ourselves pushed), OR
 *  - the explicit Effect parent's `spanId` matches the host's current
 *    span (sequential / nested case).
 *
 * Otherwise (e.g. an explicit external span parent that doesn't match
 * the host stack, or a parent set by a forked fiber whose host stack
 * has drifted) we fall back to a {@link Tracer.NativeSpan} so the
 * in-Effect span tree stays coherent without corrupting the host
 * stack.
 *
 * Note: `root: true` set by the user is intentionally ignored when
 * `parent` is None — Effect normalizes `root` to `Option.isNone(parent)`
 * for top-level spans, so we can't tell explicit-root from
 * default-root apart at this layer. In Golem the host invocation
 * context is the canonical root for a worker call, so silently
 * attaching to it is the correct behaviour for the common case.
 */
const isHostStackSafe = (host, parent) => {
    if (Option.isNone(parent))
        return true;
    try {
        const ctx = host.currentContext();
        return parent.value.spanId === ctx.spanId() && parent.value.traceId === ctx.traceId();
    }
    catch {
        return false;
    }
};
/**
 * Effect `Span` implementation that delegates to a host `Span` resource
 * handle. Local state is kept for everything the host doesn't model
 * (events, links, status) so reading code keeps working.
 */
class GolemSpan {
    handle;
    name;
    spanId;
    traceId;
    parent;
    annotations;
    kind;
    sampled;
    _tag = "Span";
    attributes = new Map();
    events = [];
    links;
    status;
    constructor(handle, name, spanId, traceId, parent, annotations, initialLinks, kind, sampled, startTime) {
        this.handle = handle;
        this.name = name;
        this.spanId = spanId;
        this.traceId = traceId;
        this.parent = parent;
        this.annotations = annotations;
        this.kind = kind;
        this.sampled = sampled;
        this.links = [...initialLinks];
        this.status = { _tag: "Started", startTime };
    }
    end(endTime, exit) {
        if (this.status._tag === "Ended")
            return;
        this.status = {
            _tag: "Ended",
            startTime: this.status.startTime,
            endTime,
            exit,
        };
        if (Exit.isFailure(exit)) {
            try {
                this.handle.setAttribute("error", { tag: "string", val: "true" });
                this.handle.setAttribute("error.message", {
                    tag: "string",
                    val: Cause.pretty(exit.cause),
                });
            }
            catch {
                // best-effort
            }
        }
        try {
            this.handle.finish();
        }
        catch {
            // best-effort: never let telemetry break business logic
        }
    }
    attribute(key, value) {
        this.attributes.set(key, value);
        try {
            this.handle.setAttribute(key, toAttributeValue(value));
        }
        catch {
            // best-effort
        }
    }
    event(name, startTime, attributes) {
        this.events.push([name, startTime, attributes ?? {}]);
    }
    addLinks(links) {
        for (const l of links)
            this.links.push(l);
    }
}
// ---------------------------------------------------------------------------
// Tracer factory
// ---------------------------------------------------------------------------
/**
 * Build the Effect `Tracer` backed by the given {@link TracingHost}.
 * Each `withSpan` call either delegates to the host (sequential /
 * nested case) or falls back to {@link Tracer.NativeSpan}
 * (forked / explicit-parent case). All operations are best-effort —
 * host failures degrade gracefully to local-only tracing.
 */
const makeGolemTracer = (host) => Tracer.make({
    span({ annotations, kind, links, name, parent, sampled, startTime }) {
        if (!isHostStackSafe(host, parent)) {
            return new Tracer.NativeSpan({
                name,
                parent,
                annotations,
                links: [...links],
                startTime,
                kind,
                sampled,
            });
        }
        let handle;
        try {
            handle = host.startSpan(name);
        }
        catch {
            return new Tracer.NativeSpan({
                name,
                parent,
                annotations,
                links: [...links],
                startTime,
                kind,
                sampled,
            });
        }
        let spanId = ZERO_SPAN_ID;
        let traceId = ZERO_TRACE_ID;
        try {
            const ctx = host.currentContext();
            spanId = ctx.spanId();
            traceId = ctx.traceId();
        }
        catch {
            // host failure: keep zero ids but still produce a working span
        }
        return new GolemSpan(handle, name, spanId, traceId, parent, annotations, links, kind, sampled, startTime);
    },
});
// ---------------------------------------------------------------------------
// Layer
// ---------------------------------------------------------------------------
/**
 * Provide the Golem host tracer for the wrapped program. Every
 * `Effect.withSpan` (and the built-in span events emitted by
 * `Logger.tracerLogger`, when enabled) flows through it.
 *
 * @since 1.5.0
 * @category layers
 */
const layer = Layer.effect(Tracer.Tracer, Effect.gen(function* () {
    const host = yield* TracingHost;
    return makeGolemTracer(host);
}));
const snapshotOf = (host) => {
    const ctx = host.currentContext();
    return {
        traceId: ctx.traceId(),
        spanId: ctx.spanId(),
        traceContextHeaders: ctx.traceContextHeaders().map(([k, v]) => [k, v]),
    };
};
/**
 * Read the host's current invocation context, wrapped in Effect.
 *
 * @since 1.5.0
 * @category getters
 */
Effect.gen(function* () {
    const host = yield* TracingHost;
    return yield* Effect.try({
        try: () => snapshotOf(host),
        catch: (e) => new TracingHostError(e),
    });
});
/**
 * Read the W3C Trace Context headers for the current invocation.
 *
 * @since 1.5.0
 * @category getters
 */
Effect.gen(function* () {
    const host = yield* TracingHost;
    return yield* Effect.try({
        try: () => host
            .currentContext()
            .traceContextHeaders()
            .map(([k, v]) => [k, v]),
        catch: (e) => new TracingHostError(e),
    });
});
// ---------------------------------------------------------------------------
// Invocation-root parent injection
// ---------------------------------------------------------------------------
/**
 * Capture the host's current invocation context and wrap `effect` so
 * that every internally-created Effect span has the host root as its
 * parent. This makes the in-Effect span tree align with the live
 * invocation context without producing an extra child host span.
 *
 * Best-effort: if reading the host fails, we just run `effect`.
 *
 * Effect-typed: requires {@link TracingHost} so that the host read
 * happens *inside* the layer scope (the dispatcher applies this
 * combinator before `Effect.provide(userRuntimeLayer)` strips the
 * dependency).
 *
 * @since 1.5.0
 * @category combinators
 */
const withInvocationParent = (effect) => Effect.gen(function* () {
    const host = yield* TracingHost;
    let snap;
    try {
        snap = snapshotOf(host);
    }
    catch {
        return yield* effect;
    }
    if (snap.traceId === ZERO_TRACE_ID || snap.spanId === ZERO_SPAN_ID) {
        return yield* effect;
    }
    const external = Tracer.externalSpan({
        traceId: snap.traceId,
        spanId: snap.spanId,
        sampled: true,
    });
    return yield* Effect.withParentSpan(effect, external);
});

/**
 * @since 1.5.0
 */
/**
 * Combined Logger + Tracer layer applied automatically to every piece
 * of user code the dispatcher runs (`impl`, method handlers, custom
 * snapshot save/load). Routes `Effect.log*` to `wasi:logging` and
 * `Effect.withSpan` to `golem:api/context`.
 */
const observabilityLayer = Layer.mergeAll(layer$1, layer);
/**
 * The runtime layer applied to every piece of user code the dispatcher
 * runs. Provides:
 * - All host services (env, config, …) via {@link HostLive} so user
 *   effects that yield from a host-service tag (e.g. `ConfigClient`)
 *   resolve against the real WIT specifier in production and against
 *   test fakes in tests.
 * - {@link observabilityLayer} on top, so `Effect.log*` /
 *   `Effect.withSpan` route through the host bindings.
 *
 * `Layer.provideMerge` is used (not `Layer.mergeAll`) so that
 * {@link observabilityLayer}'s dependency on `LoggingHost` and
 * `TracingHost` is satisfied by `HostLive`. The result also re-merges
 * `HostLive` into its output, so user code that yields a host-service
 * tag directly resolves the same way.
 *
 * Built once at module load (matches the previous `observabilityLayer`
 * pattern); per-invocation cost is zero allocations.
 */
const userRuntimeLayer = Layer.provideMerge(observabilityLayer, HostLive);
/**
 * Module-level `ManagedRuntime` backed by {@link userRuntimeLayer}.
 * Built lazily on first use; reused across every `dispatch*` entry
 * point (`initialize`, `invoke`, `save-snapshot`, `load-snapshot`).
 * This is what makes the "host services constructed once per process"
 * claim mechanically true: `Layer.scoped` acquire-counters fire
 * exactly once across multiple invocations.
 */
const userRuntime = ManagedRuntime.make(userRuntimeLayer);
/**
 * Run a user effect on the cached {@link userRuntime}. Reuses the
 * pre-built layer context across every `dispatch*` entry point, so
 * host services aren't reconstructed per dispatch.
 *
 * `withInvocationParent` is applied *first* (widening the inner
 * effect's `R` channel with `TracingHost`); the runtime then
 * fulfills `TracingHost` via `HostLive`. This is required because
 * `withInvocationParent` reads the host's invocation context via the
 * `TracingHost` service.
 */
const runUserPromise = (eff) => userRuntime.runPromise(withCustomScopes(withInvocationParent(eff)));
/**
 * Define an agent type and build a typed RPC client for it WITHOUT
 * registering the agent with the runtime.
 *
 * **Why the split**
 *
 * The returned {@link AgentSpec} is safe to import from modules whose
 * only job is to make remote calls to this agent type via
 * `spec.client.*`. Importing a spec module does not pull in the
 * implementation code or trigger any registration side effect on the
 * host. Registration happens when {@link AgentSpec.implement} is
 * called — typically from the component's entry point.
 *
 * **Canonicalization**
 *
 * `defineAgent` shallow-clones and freezes the supplied metadata
 * (top-level object, `constructorParams`, `methods`) so that the
 * spec's `client` (built immediately) and any later
 * `spec.implement(...)` registration always agree on the same view of
 * the metadata. Mutating the original user-supplied literal after
 * `defineAgent` returns has no effect on the spec.
 *
 * **Compile-time guarantees on the `http` field**
 *
 * When at least one method in `methods` declares an `http` array of
 * endpoints, the agent's `http: Http.mount(...)` field becomes
 * *required* — `tsc` rejects the call site with "Property 'http' is
 * missing" when it is omitted. When no method declares HTTP
 * endpoints, `http` remains optional, matching the runtime "any
 * endpoints declared" check in `validateAgentHttp`.
 *
 * When `http` IS supplied, two additional type-level constraints
 * apply to it:
 *
 * - Every constructor parameter in `constructorParams` must appear as
 *   a `{var}` segment in the mount path; missing vars surface as an
 *   `Invalid<"mount path missing var '…'">` carrier on the assigned
 *   {@link MountDefCovering} type.
 * - Every `{var}` in the optional `webhookSuffix` must match a
 *   constructor-parameter name AND must be statically eligible for
 *   binding (i.e. NOT a {@link Multimodal} or {@link ElementSpec}
 *   carrier — see {@link BindableKeys}); violations surface as a
 *   {@link WebhookVarsValid} carrier with a readable reason string.
 *
 * Trivial path-shape rules and per-endpoint duplicate-binding /
 * case-fold / bodyless-unbound checks fire earlier — at the
 * `Http.mount(...)` / `Http.get(...)` / `method({ http: [...] })`
 * call sites.
 *
 * **Runtime fallbacks (defence-in-depth)**
 *
 * Full string-bindability of mount-path / webhook-suffix vars,
 * the brace-balance check, the var-name regex, and any
 * configuration whose path string was supplied as a non-literal
 * `string` value still run inside `validateAgentHttp` /
 * `validateMount` / `validateEndpoint` and surface as `HttpRouteError`
 * from `spec.implement(...)`.
 *
 * **Validation-error reporting**
 *
 * Validation failures from a later `spec.implement(...)` call (i.e.
 * `UnsupportedSchemaError`, `HttpRouteError`, `InvalidSnapshotError`,
 * `DuplicateAgentNameError`, or any other typed failure / defect)
 * are NOT thrown synchronously. Instead the failure is captured in
 * {@link pendingRegistrationErrors} and re-emitted as a typed
 * `golem:agent/common@2.0.0.agent-error` (`invalid-type` variant)
 * thrown from the WIT-exported
 * {@link dispatchDiscoverAgentTypes} host call. This lets the
 * Golem CLI's metadata-extraction step surface misconfigurations
 * as proper structured diagnostics rather than as a WASM
 * instantiation crash. The returned {@link ImplementedAgent} value
 * is still constructed so that other modules importing the agent
 * keep working — any subsequent `initialize` / `invoke` against the
 * un-registered type still fails at the dispatcher with the usual
 * "unknown agent" error.
 *
 * @see {@link registerAgent} for the lower-level registration-only
 *      entry point (Effect-typed failures).
 *
 * @since 1.5.0
 * @category constructors
 */
const defineAgent = (metadata) => {
    // Canonicalize: shallow-clone the top-level object and the two
    // nested containers, then freeze them so the spec is immutable from
    // the caller's perspective. This closes the mutation window between
    // spec construction (which `clientFor` reads from) and
    // `.implement(...)` (which `registerAgent` re-reads from later).
    const canonicalId = Object.freeze({ ...metadata.id });
    const canonicalMethods = Object.freeze({ ...metadata.methods });
    const canonical = Object.freeze({
        ...metadata,
        id: canonicalId,
        methods: canonicalMethods,
    });
    // Build the typed RPC client once from the canonical metadata. The
    // same reference is shared between the spec and any
    // `ImplementedAgent` produced by `spec.implement(...)` below.
    const sharedClient = clientFor(canonical);
    const sharedBinding = bindingFor(canonical);
    let identityCodec;
    const agentId = (input, phantomId) => Effect.gen(function* () {
        if (canonical.mode === "ephemeral" && phantomId === undefined)
            return yield* Effect.fail(new AgentIdentityError(new TypeError(`ephemeral agent type '${canonical.name}' requires a phantom ID`)));
        const codec = (identityCodec ??= yield* compileCallerParamBindings(`${canonical.name} constructor`, canonical.id));
        const constructorValue = yield* codec
            .encodeAsync(input)
            .pipe(Effect.mapError((cause) => new AgentIdentityError(cause)));
        return yield* make({
            typeName: canonical.name,
            constructorValue,
            ...(phantomId === undefined ? {} : { phantomId }),
        });
    });
    // `implement` is created as a closure rather than a prototype method
    // so the generics inferred by `defineAgent` (C, Methods, M, F, S)
    // flow into the constructor parameter shape without requiring the
    // user to re-state them.
    //
    // `.implement(...)` is single-shot per spec — the `consumed` flag is
    // set on the first call regardless of whether registration succeeded
    // or pushed a deferred error. A second call always becomes a deferred
    // {@link DuplicateAgentNameError} so a flaky retry loop cannot leak
    // additional registrations or accumulate stacked errors.
    let consumed = false;
    const implement = (implementation) => {
        if (consumed) {
            pendingRegistrationErrors.push({
                agentName: canonical.name,
                cause: Cause.fail(new DuplicateAgentNameError(canonical.name)),
            });
        }
        else {
            consumed = true;
            // The caller-side `AgentHttpRequirement` intersection is already
            // satisfied at the `defineAgent` call site; re-introduce it here
            // for `registerAgent`'s strictly-typed input.
            const metadataForRegistration = canonical;
            const exit = Effect.runSyncExit(registerAgent(metadataForRegistration, implementation));
            if (Exit.isFailure(exit)) {
                pendingRegistrationErrors.push({ agentName: canonical.name, cause: exit.cause });
            }
        }
        // Erase the `MV` / `WV` mount-vars phantoms — the public
        // {@link ImplementedAgent} surfaces only the user-visible fields.
        return Object.freeze({
            ...canonical,
            client: sharedClient,
            agentId,
            [bindIdentity]: sharedBinding[bindIdentity],
            spec,
        });
    };
    // Erase the `MV` / `WV` phantoms as above; the public
    // {@link AgentSpec} surface is generic only over the user-visible
    // parameters.
    const spec = Object.freeze({
        ...canonical,
        client: sharedClient,
        agentId,
        [bindIdentity]: sharedBinding[bindIdentity],
        implement,
    });
    return spec;
};
/** Module-level registry of compiled agents, keyed by `typeName`. */
const registry = new Map();
/**
 * Validation failures captured by {@link defineAgent} (i.e. typed
 * failures or defects from {@link registerAgent} — invalid schemas,
 * malformed HTTP routes, malformed snapshot config, duplicate agent
 * names, etc.). These are deliberately NOT thrown at module-import
 * time; instead they are surfaced from the WIT-exported
 * `discover-agent-types` host call as a typed `AgentError` so that
 * tooling (e.g. the Golem CLI's metadata-extraction pass) can present
 * them as structured diagnostics rather than as a WASM instantiation
 * crash.
 */
const pendingRegistrationErrors = [];
/**
 * Raised by {@link registerAgent} when an agent type name is registered
 * more than once in the same component. The SDK rejects the second
 * registration fail-fast rather than silently overwriting the earlier
 * definition, because two `defineAgent` calls sharing a `name` would
 * otherwise leave `discoverAgentTypes` / `initialize` operating on
 * whichever module happened to be imported last.
 *
 * `defineAgent` does NOT throw this error synchronously: it stashes
 * the failure in {@link pendingRegistrationErrors} and re-emits it as
 * a typed `agent-error` (`invalid-type` variant) from
 * {@link dispatchDiscoverAgentTypes} so the host / CLI can surface
 * the conflict as a structured diagnostic.
 *
 * @since 1.5.0
 * @category errors
 */
class DuplicateAgentNameError {
    agentName;
    _tag = "DuplicateAgentNameError";
    message;
    constructor(agentName) {
        this.agentName = agentName;
        this.message = `DuplicateAgentNameError: an agent named '${agentName}' is already registered in this component`;
    }
}
/**
 * Register an agent metadata + implementation pair with the runtime.
 * Pure schema-walking work for the metadata side — `impl` is captured
 * but NOT executed here; no per-instance state is created. Safe to
 * call at deploy time for type discovery.
 *
 * Surfaces validation failures as typed Effect failures, unlike the
 * chained `defineAgent(...).implement(...)` form, which defers them
 * to {@link dispatchDiscoverAgentTypes}.
 *
 * @see {@link defineAgent} for the chained-API shorthand that additionally
 *      builds a typed RPC client.
 *
 * @since 1.5.0
 * @category constructors
 */
const registerAgent = (metadata, impl, routerHttp) => Effect.gen(function* () {
    if (registry.has(metadata.name)) {
        return yield* Effect.fail(new DuplicateAgentNameError(metadata.name));
    }
    const constructorCodec = yield* compileParamBindings(`${metadata.name} constructor`, metadata.id);
    const methodCodecs = new Map();
    const methodHttpInputs = [];
    for (const [methodName, spec] of Object.entries(metadata.methods)) {
        const mc = (yield* compileMethodSpec(methodName, spec));
        methodCodecs.set(methodName, mc);
        methodHttpInputs.push({
            name: methodName,
            params: Object.fromEntries(Object.entries(spec.input).filter(([name]) => mc.inputSchema.tag !== "parameters" ||
                mc.inputSchema.val.some((field) => field.name === name && field.source.tag === "user-supplied"))),
            endpoints: spec.http ?? [],
            nonStringBindableParams: collectNonStringBindableParams(spec.input),
            stringBindableParams: collectStringBindableParams(spec.input),
            queryOrHeaderBindableParams: collectQueryOrHeaderBindableParams(spec.input),
            ...collectHttpQueryParams(mc.inputCodec.graph),
        });
    }
    // Validate + compile HTTP routes (mount + per-method endpoints).
    if (metadata.http?.phantomId && metadata.mode === "ephemeral") {
        return yield* Effect.fail(new HttpRouteError("Phantom selectors require a regular durable agent"));
    }
    if (metadata.http?.exposeFiles?.length &&
        (metadata.mode === "ephemeral" || metadata.http.phantomAgent)) {
        return yield* Effect.fail(new HttpRouteError("File exposure requires a regular durable non-phantom agent"));
    }
    const compiledHttp = (yield* validateAgentHttp({
            agentName: metadata.name,
            mount: metadata.http,
            constructorParamNames: Object.keys(metadata.id),
            nonStringBindableConstructorParams: collectNonStringBindableParams(metadata.id),
            stringBindableConstructorParams: collectStringBindableParams(metadata.id),
            methods: methodHttpInputs,
        }));
    let compiledConfig = null;
    if (metadata.config !== undefined)
        compiledConfig = yield* metadata.config.__compile();
    const graphs = [
        constructorCodec.graph,
        ...Array.from(methodCodecs.values(), (codec) => codec.graph),
        ...(compiledConfig?.graphs ?? []),
    ];
    const encoder = new GraphEncoder(mergeGraphDefs(graphs));
    const inputSchema = (codec) => {
        const fields = codec.graph.root.body.tag === "record" ? codec.graph.root.body.fields : [];
        return {
            ...codec.inputSchema,
            val: codec.inputSchema.val.map((field, index) => ({
                ...field,
                schema: encoder.encodeType(fields[index].body),
            })),
        };
    };
    // Now build the AgentMethod records, attaching the compiled
    // httpEndpoint list per method.
    const agentMethods = [];
    for (const [methodName, spec] of Object.entries(metadata.methods)) {
        const mc = methodCodecs.get(methodName);
        const eps = compiledHttp.endpoints.get(methodName) ?? [];
        agentMethods.push({
            name: methodName,
            description: spec.description ?? "",
            httpEndpoint: [...eps],
            promptHint: spec.promptHint,
            inputSchema: inputSchema(mc),
            outputSchema: mc.outputCodec === undefined
                ? { tag: "unit" }
                : { tag: "single", val: encoder.encodeType(mc.outputCodec.graph.root) },
            readOnly: mc.readOnly,
        });
    }
    const configDeclarations = compiledConfig?.declarations(compiledConfig.graphs.map((graph) => encoder.encodeType(graph.root))) ?? [];
    let compiledSnapshot = null;
    let snapshotting = { tag: "disabled" };
    if (metadata.snapshotting !== undefined) {
        if (metadata.mode === "ephemeral") {
            return yield* Effect.fail(new InvalidSnapshotError(`agent '${metadata.name}' is ephemeral and declares snapshotting, but ephemeral ` +
                "agents take no snapshots. Remove the snapshotting option or make the agent durable."));
        }
        const cs = yield* compileSnapshot(metadata.name, metadata.snapshotting);
        if (cs.kind === "multipart" &&
            (typeof impl.snapshot?.save !== "function" || typeof impl.snapshot?.restore !== "function")) {
            return yield* Effect.fail(new InvalidSnapshotError(`agent '${metadata.name}' multipart snapshot requires save and restore strategies`));
        }
        compiledSnapshot = cs;
        snapshotting = { tag: "enabled", val: cs.witConfig };
    }
    const agentType = {
        typeName: metadata.name,
        kind: "regular",
        description: metadata.description ?? "",
        sourceLanguage: "typescript",
        schema: encoder.finish(),
        constructor: {
            name: undefined,
            description: "",
            promptHint: metadata.promptHint,
            inputSchema: inputSchema(constructorCodec),
        },
        methods: agentMethods,
        dependencies: [...(metadata.dependencies ?? [])],
        mode: metadata.mode ?? "durable",
        httpMount: compiledHttp.mount,
        snapshotting,
        config: [...configDeclarations],
    };
    registry.set(metadata.name, {
        name: metadata.name,
        metadata: metadata,
        impl: impl,
        constructorCodec: constructorCodec,
        methodCodecs,
        agentType,
        compiledConfig,
        compiledSnapshot,
    });
});
const collectHttpQueryParams = (graph) => {
    const queryBindableParams = new Set();
    const streamParams = new Set();
    const bindable = (type, collections = true) => {
        const shape = resolveShapeType(graph, type);
        if (!shape)
            return false;
        switch (shape.tag) {
            case "bool":
            case "s8":
            case "s16":
            case "s32":
            case "s64":
            case "u8":
            case "u16":
            case "u32":
            case "u64":
            case "f32":
            case "f64":
            case "char":
            case "string":
            case "enum":
                return true;
            case "option":
                return collections && bindable(shape.element);
            case "list":
                return collections && bindable(shape.element, false);
            case "map":
                return collections && bindable(shape.key, false) && bindable(shape.value, false);
            default:
                return false;
        }
    };
    const root = resolveShapeType(graph, graph.root);
    if (root?.tag === "record") {
        for (const field of root.fields) {
            if (bindable(field.body))
                queryBindableParams.add(field.name);
            if (resolveShapeType(graph, field.body)?.tag === "stream")
                streamParams.add(field.name);
        }
    }
    return { queryBindableParams, streamParams };
};
const collectNonStringBindableParams = (params) => {
    const out = new Set();
    for (const [name, p] of Object.entries(params)) {
        if (isMultimodal(p) || isElementSpec(p)) {
            out.add(name);
        }
    }
    return out;
};
const collectStringBindableParams = (params) => collectBindableParams(params, isStringBindableSchema);
const collectQueryOrHeaderBindableParams = (params) => collectBindableParams(params, isQueryOrHeaderBindableSchema);
const collectBindableParams = (params, isBindable) => {
    const out = new Set();
    for (const [name, p] of Object.entries(params)) {
        if (isMultimodal(p) || isElementSpec(p))
            continue;
        // Only Schema.Top values can be string-bindable.
        if (Schema.isSchema(p)) {
            if (isBindable(p)) {
                out.add(name);
            }
        }
    }
    return out;
};
let activeAgent = null;
/** Decode an incoming constructor-input `DataValue` into a record of
 *  decoded parameter values, in the same way both `initialize` and
 *  `load` need to. */
const decodeConstructorInput = async (compiled, input) => {
    return await Effect.runPromise(compiled.constructorCodec.decode(input));
};
/**
 * Open a fresh scope, run `impl` with the supplied principal/config
 * service, and return the resulting handlers + (optional) bound
 * snapshot. Used by both `dispatchInitialize` and `dispatchLoadSnapshot`
 * so the two share a single code path for everything except how state
 * is restored after `impl` returns.
 */
const initAgentInstance = async (agentTypeName, compiled, constructorInput, principal, restoration) => {
    let snapshot = null;
    const scope = await Effect.runPromise(Scope.make());
    let handlers;
    let state;
    let configShape = undefined;
    try {
        if (compiled.compiledConfig !== null) {
            configShape = await runUserPromise(compiled.compiledConfig.buildShape());
        }
        const stateProgram = restoration === undefined
            ? compiled.impl.init(constructorInput)
            : compiled.impl.snapshot === undefined
                ? Effect.succeed(restoration.saved)
                : compiled.impl.snapshot.restore(restoration.saved, {
                    id: constructorInput,
                    principal,
                    phantomId: restoration.phantomId,
                    agentId: await runUserPromise(SelfAgentId),
                    parsedAgentId: restoration.parsedAgentId,
                    config: configShape,
                });
        let program = Effect.gen(function* () {
            const state = yield* stateProgram;
            if (compiled.compiledSnapshot !== null) {
                snapshot = createSnapshot(agentTypeName, compiled.compiledSnapshot, compiled.impl.snapshot?.databases?.(state));
                if (restoration !== undefined) {
                    restoration.restoreDatabases(snapshot);
                }
            }
            const produced = compiled.impl.methods(state);
            const handlers = Effect.isEffect(produced) ? yield* produced : produced;
            return { state, handlers };
        }).pipe(Effect.provideService(Principal, principal));
        if (compiled.compiledConfig !== null && compiled.metadata.config !== undefined) {
            program = program.pipe(
            // The config class is a Context.Service tag (Self/Identifier
            // resolved via the user's `defineConfig`-class declaration). We
            // erase the static generics here because the dispatcher works
            // generically over every registered agent.
            Effect.provideService(compiled.metadata.config, configShape));
        }
        // Use `Scope.provide` (NOT `Scope.use`) — the latter auto-closes
        // the scope when `program` finishes, which would tear down any
        // resources `impl` opened (e.g. SqliteClient handles) before the
        // agent's first method invocation.
        const built = await runUserPromise(Scope.provide(program, scope));
        handlers = built.handlers;
        state = built.state;
    }
    catch (e) {
        // Initialization failed; close the scope to release anything that
        // managed to be acquired before the failure.
        await Effect.runPromise(Scope.close(scope, Exit.void));
        throw e;
    }
    return { scope, handlers, snapshot, configShape, state: state };
};
/**
 * Implementation of `agent-guest.guest.initialize`.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
const dispatchInitialize = async (agentTypeName, input) => {
    const compiled = registry.get(agentTypeName);
    if (!compiled) {
        throw new Error(`unknown agent: ${agentTypeName}; registered: ${[...registry.keys()].join(", ") || "<none>"}`);
    }
    if (activeAgent !== null) {
        throw new Error(`agent already initialized in this container: ${activeAgent.name}`);
    }
    const principal = await runUserPromise(immediateCaller);
    const constructorInput = await decodeConstructorInput(compiled, input);
    const { scope, handlers, snapshot, state } = await initAgentInstance(agentTypeName, compiled, constructorInput, principal);
    activeAgent = { name: agentTypeName, scope, handlers, principal, snapshot, state };
};
/**
 * Implementation of `agent-guest.guest.invoke`.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
const dispatchInvoke = async (methodName, input) => {
    if (activeAgent === null) {
        throw new Error(`agent is not initialized; cannot invoke ${methodName}`);
    }
    const compiled = registry.get(activeAgent.name);
    const mc = compiled.methodCodecs.get(methodName);
    if (!mc) {
        throw new Error(`unknown method '${methodName}' on agent '${activeAgent.name}'; available: ${[...compiled.methodCodecs.keys()].join(", ") || "<none>"}`);
    }
    const handler = activeAgent.handlers[methodName];
    if (!handler) {
        throw new Error(`agent '${activeAgent.name}' did not provide an implementation for '${methodName}'`);
    }
    // Provide the per-call principal as an Effect service so handler
    // bodies can read the *current* caller via `yield* Principal` (which
    // may differ from the initialize-time principal stored on
    // `activeAgent`). The optional config service is rebuilt fresh per
    // invocation: regular fields are memoized for the duration of THIS
    // call only; secret fields are never cached.
    let program = invokeWireValue(mc.inputCodec, mc.encodeOutput, {
        errorWrapped: mc.errorWrapped,
        successVoid: mc.successVoid,
        inputSchema: compiled.agentType.methods.find((method) => method.name === methodName)
            .inputSchema,
    }, (decoded) => Effect.suspend(() => handler(decoded)).pipe(Effect.catchCause((cause) => {
        // Declared failures remain result values. Interruption stays separate
        // from application errors for cooperative cancellation.
        if (Cause.hasInterrupts(cause) || (mc.errorWrapped && !Cause.hasDies(cause))) {
            return Effect.failCause(cause);
        }
        return Effect.die({
            tag: "custom-error",
            val: typedSchemaValueToWit({
                graph: { defs: new Map(), root: t.string() },
                value: v.string(Cause.pretty(cause)),
            }),
        });
    })), input);
    if (mc.readOnly === undefined || mc.readOnly.usesPrincipal) {
        const principal = await runUserPromise(immediateCaller);
        program = program.pipe(Effect.provideService(Principal, principal));
    }
    if (compiled.compiledConfig !== null && compiled.metadata.config !== undefined) {
        const shape = await runUserPromise(compiled.compiledConfig.buildShape());
        program = program.pipe(Effect.provideService(compiled.metadata.config, shape));
    }
    if (compiled.agentType.kind === "http-router") {
        const scope = Scope.makeUnsafe();
        const handler = compiled.agentType.methods.find((method) => method.name === methodName);
        const transfersScope = handler?.httpEndpoint.some((endpoint) => endpoint.httpMethod.tag === "any");
        return await runUserPromise(program.pipe(Scope.provide(scope), Effect.onExit((exit) => !transfersScope || Exit.isFailure(exit) ? Scope.close(scope, exit) : Effect.void)));
    }
    return await runUserPromise(program);
};
/**
 * Implementation of `agent-guest.guest.discoverAgentTypes`.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
const dispatchDiscoverAgentTypes = () => {
    if (pendingRegistrationErrors.length > 0) {
        throw makeRegistrationAgentError(pendingRegistrationErrors);
    }
    return Array.from(registry.values()).map((c) => c.agentType);
};
/**
 * Render the stashed `defineAgent` registration failures as the
 * `invalid-type` variant of the WIT
 * `golem:agent/common@2.0.0.agent-error` ADT — that's the closest
 * variant to "this component's declared agent types could not be
 * built". The `val` string concatenates one section per failing agent
 * (the agent name, plus `Cause.pretty` of its underlying Effect
 * cause), giving the Golem CLI / host a human-readable diagnostic
 * payload while staying within the host-defined `string`-typed
 * variant.
 */
const makeRegistrationAgentError = (errors) => {
    const sections = errors.map(({ agentName, cause }) => `agent '${agentName}': ${Cause.pretty(cause)}`);
    const header = errors.length === 1
        ? "effect-golem: 1 agent registration error"
        : `effect-golem: ${errors.length} agent registration errors`;
    const message = `${header}\n${sections.join("\n")}`;
    return { tag: "invalid-type", val: message };
};
/**
 * Implementation of `agent-guest.guest.getDefinition`.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
const dispatchGetDefinition = () => {
    if (activeAgent === null) {
        throw new Error("agent is not initialized; cannot get definition");
    }
    return registry.get(activeAgent.name).agentType;
};
// ---------------------------------------------------------------------------
// Snapshotting dispatchers
// ---------------------------------------------------------------------------
/**
 * Resolve the `SqliteHostExtClient` impl out of the production
 * {@link userRuntimeLayer} synchronously. `userRuntime.runSync` is
 * safe here because the underlying layers (`SqliteHostExtLive` etc.)
 * are pure `Layer.succeed`s — no async work happens during resolution.
 */
const resolveSqliteHostExtSync = () => userRuntime.runSync(Effect.gen(function* () {
    const ext = yield* SqliteHostExtClient;
    return {
        serializeDatabaseSync: ext.serializeDatabaseSync,
        restoreDatabaseSync: ext.restoreDatabaseSync,
        isAutocommitDatabaseSync: ext.isAutocommitDatabaseSync,
    };
}));
/** Validate the same managed-connection restrictions during save and restoration. */
const snapshotDatabaseLocation = (agentName, name, handle, sqliteExt) => {
    if (!handle.isOpen)
        throw new SnapshotEnvelopeError(`agent '${agentName}' database '${name}' is closed`);
    if (!sqliteExt.isAutocommitDatabaseSync(handle))
        throw new SnapshotDatabaseNotInAutocommitError(agentName, name);
    const rows = handle.prepare("PRAGMA database_list").all();
    const extra = rows
        .map((row) => String(Array.isArray(row) ? row[1] : row.name))
        .filter((schema) => schema !== "main" && schema !== "temp");
    if (extra.length > 0)
        throw new SnapshotDatabaseHasAttachmentsError(agentName, name, extra);
    return handle.location();
};
/** Rebuild the connection cache before recorded suffix replay begins. */
const warmSnapshotDatabase = (handle, fileBacked, sqliteExt) => {
    handle.prepare("SELECT count(*) FROM sqlite_master").get();
    if (!fileBacked)
        return;
    const pragma = (name) => {
        const row = handle.prepare(`PRAGMA ${name}`).get();
        return Number(Array.isArray(row) ? row[0] : row?.[name]);
    };
    const pageCount = pragma("page_count");
    const pageSize = pragma("page_size");
    const cacheSize = pragma("cache_size");
    // wasm32 SQLite keeps 88 bytes of page metadata and recycles before the cache limit.
    const limit = cacheSize < 0 ? Math.floor((-cacheSize * 1024) / (pageSize + 88)) : cacheSize;
    if (pageCount > 0 && pageCount <= limit - 1) {
        // Discarded read for cache warming, not a captured application snapshot image.
        sqliteExt.serializeDatabaseSync(handle);
    }
};
/**
 * Encode the schema-driven snapshot state and optional SQLite images.
 */
const encodeAutoSnapshot = async (agent, compiled, snap) => {
    const saved = compiled.impl.snapshot === undefined ? agent.state : await saveSnapshotState(agent, compiled);
    const bundle = snap.kind === "multipart"
        ? saved
        : undefined;
    if (snap.kind === "multipart" && !(bundle?.parts instanceof Map)) {
        throw new InvalidSnapshotError(`agent '${agent.name}' multipart save must return a state and a Map of parts`);
    }
    const state = bundle === undefined ? saved : bundle.state;
    const encoded = Effect.runSync(Schema.encodeUnknownEffect(snap.schema)(state));
    const encodedState = encodeSnapshotState(encoded);
    if (snap.kind === "auto" && snap.declaredDatabases.length === 0) {
        return encodeJsonEnvelope(agent.principal, encodedState);
    }
    const sqliteExt = snap.declaredDatabases.length > 0 ? resolveSqliteHostExtSync() : undefined;
    const dbParts = [];
    const fileDatabases = [];
    for (const dbName of snap.declaredDatabases) {
        const handle = snap.databases.get(dbName);
        if (handle === undefined) {
            throw new SnapshotDatabaseMissingPartError(agent.name, dbName, "save");
        }
        const location = snapshotDatabaseLocation(agent.name, dbName, handle, sqliteExt);
        if (location === null)
            dbParts.push({ name: dbName, bytes: sqliteExt.serializeDatabaseSync(handle) });
        else
            fileDatabases.push([dbName, location]);
    }
    return encodeMultipartJsonEnvelope(agent.principal, encodedState, dbParts, bundle?.parts, snap.declaredDatabases.length > 0 ? Object.fromEntries(fileDatabases) : undefined);
};
/**
 * Implementation of `golem:api/save-snapshot.save`. Reads the active
 * agent's bound snapshot state, encodes it according to the active
 * variant (auto → JSON envelope or multipart/mixed when SQLite databases
 * are declared; custom → binary v2 envelope), and returns the resulting
 * `Snapshot` to the host.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
const dispatchSaveSnapshot = async () => {
    if (activeAgent === null) {
        throw new Error("agent is not initialized; cannot save snapshot");
    }
    const agent = activeAgent;
    const compiled = registry.get(agent.name);
    if (compiled.compiledSnapshot === null || agent.snapshot === null) {
        throw new Error(`agent '${agent.name}' did not declare a snapshot definition; the host should not be calling save`);
    }
    const snap = agent.snapshot;
    if (snap.kind !== "custom") {
        return encodeAutoSnapshot(agent, compiled, snap);
    }
    const bytes = await saveSnapshotState(agent, compiled);
    return encodeBinaryEnvelope(agent.principal, bytes);
};
/**
 * Run either strategy's save effect with the same services as initialization.
 */
const saveSnapshotState = async (agent, compiled) => {
    let saveProgram = Effect.suspend(() => compiled.impl.snapshot.save(agent.state)).pipe(Effect.provideService(Principal, agent.principal), Effect.scoped);
    if (compiled.compiledConfig !== null && compiled.metadata.config !== undefined) {
        const shape = await runUserPromise(compiled.compiledConfig.buildShape());
        saveProgram = saveProgram.pipe(Effect.provideService(compiled.metadata.config, shape));
    }
    return runUserPromise(saveProgram);
};
/**
 * Implementation of `golem:api/load-snapshot.load`. The host calls
 * this *instead of* `agent-guest.guest.initialize` when restoring an
 * agent from a snapshot. We:
 *
 * 1. Recover the agent's own ID from `GOLEM_AGENT_ID` and parse it.
 * 2. Decode the envelope to recover principal + user-state bytes.
 * 3. Decode state or run the strategy's restore effect, skipping `init`.
 * 4. Restore SQLite images and construct the shared methods with that state.
 * 5. Mark the agent active so subsequent `invoke`s see the restored
 *    state.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
const dispatchLoadSnapshot = async (snapshot) => {
    if (activeAgent !== null) {
        throw new Error(`agent already initialized in this container: ${activeAgent.name}`);
    }
    // 1. Recover the agent ID + parse it. Reading the WASI process env
    //    via the `EnvironmentClient` host service and parsing it via
    //    `AgentHostClient.parseAgentId` lets tests substitute fakes
    //    without monkey-patching the real specifier.
    const { env, parseAgentId } = await runUserPromise(Effect.gen(function* () {
        const ec = yield* EnvironmentClient;
        const ah = yield* AgentHostClient;
        const env = yield* ec.getEnvironment;
        return { env, parseAgentId: ah.parseAgentId };
    }));
    const agentIdEntry = env.find(([k]) => k === "GOLEM_AGENT_ID");
    if (agentIdEntry === undefined) {
        throw new Error("load-snapshot: GOLEM_AGENT_ID is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : GOLEM_AGENT_ID");
    }
    const agentIdString = agentIdEntry[1];
    const [agentTypeName, typedConstructor, phantomId] = decodeAgentId({ parseAgentId }, agentIdString);
    const compiled = registry.get(agentTypeName);
    if (!compiled) {
        throw new Error(`load-snapshot: unknown agent type '${agentTypeName}'; registered: ${[...registry.keys()].join(", ") || "<none>"}`);
    }
    if (compiled.compiledSnapshot === null) {
        throw new Error(`load-snapshot: agent '${agentTypeName}' did not declare a snapshot definition`);
    }
    // 2. Decode the envelope. We use `anonymous` as the fallback principal
    //    only for the legacy v1 binary format which carries no embedded
    //    principal — every modern envelope embeds one.
    const fallbackPrincipal = { tag: "anonymous" };
    let decoded;
    try {
        decoded = decodeEnvelope(snapshot, fallbackPrincipal);
    }
    catch (e) {
        if (e instanceof SnapshotEnvelopeError || e instanceof UnsupportedSnapshotFormatError) {
            throw e;
        }
        throw e;
    }
    const principal = decoded.principal;
    let saved;
    if (compiled.compiledSnapshot.kind !== "custom") {
        if (decoded.kind !== "json" && decoded.kind !== "multipart") {
            throw new SnapshotEnvelopeError(`agent '${agentTypeName}' expects a JSON snapshot state but received ${snapshot.mimeType}`);
        }
        const config = compiled.compiledSnapshot;
        const expectsMultipart = config.kind === "multipart" || config.declaredDatabases.length > 0;
        if ((decoded.kind === "multipart") !== expectsMultipart) {
            throw new SnapshotEnvelopeError(`agent '${agentTypeName}' expects ${expectsMultipart ? "multipart/mixed" : "JSON"} envelope`);
        }
        if (decoded.kind === "multipart") {
            if (config.kind === "auto" && decoded.parts.size > 0) {
                throw new SnapshotEnvelopeError(`agent '${agentTypeName}' simple snapshot cannot restore user parts`);
            }
            const declared = new Set(config.declaredDatabases);
            const received = new Set([
                ...decoded.databases.map((part) => part.name),
                ...Object.keys(decoded.fileDatabases ?? {}),
            ]);
            for (const name of received) {
                if (!declared.has(name))
                    throw new SnapshotDatabaseUnknownPartError(agentTypeName, name);
            }
            for (const name of declared) {
                if (!received.has(name))
                    throw new SnapshotDatabaseMissingPartError(agentTypeName, name, "load-envelope");
            }
            if (declared.size > 0 && decoded.fileDatabases === undefined)
                throw new SnapshotEnvelopeError(`agent '${agentTypeName}' managed snapshot missing 'fileDatabases'`);
            for (const [name, location] of Object.entries(decoded.fileDatabases ?? {})) {
                if (!existsSync(location))
                    throw new SnapshotEnvelopeError(`agent '${agentTypeName}' database '${name}' file does not exist at '${location}'`);
            }
        }
        const state = await Effect.runPromise(Schema.decodeUnknownEffect(compiled.compiledSnapshot.schema)(decodeSnapshotState(decoded.state)));
        saved =
            config.kind === "multipart" && decoded.kind === "multipart"
                ? { state, parts: decoded.parts }
                : state;
    }
    else {
        if (decoded.kind !== "binary") {
            throw new SnapshotEnvelopeError(`agent '${agentTypeName}' expects a binary envelope but received ${snapshot.mimeType}`);
        }
        saved = decoded.userPayload;
    }
    const restoreDatabases = (bound) => {
        if (bound.kind !== "custom") {
            const declared = bound.declaredDatabases;
            if (declared.length === 0) {
                if (decoded.kind !== (bound.kind === "multipart" ? "multipart" : "json")) {
                    throw new SnapshotEnvelopeError(`agent '${agentTypeName}' expects a JSON envelope but received ${snapshot.mimeType}`);
                }
            }
            else {
                if (decoded.kind !== "multipart") {
                    throw new SnapshotEnvelopeError(`agent '${agentTypeName}' expects a multipart/mixed envelope (declared databases: ${declared.join(", ")}) but received ${snapshot.mimeType}`);
                }
                const sqliteExt = resolveSqliteHostExtSync();
                // Validate every handle before hydrating any image.
                for (const dbName of declared) {
                    const handle = bound.databases.get(dbName);
                    if (handle === undefined) {
                        throw new SnapshotDatabaseMissingPartError(agentTypeName, dbName, "load-attach");
                    }
                    const location = snapshotDatabaseLocation(agentTypeName, dbName, handle, sqliteExt);
                    const expected = Object.hasOwn(decoded.fileDatabases, dbName)
                        ? decoded.fileDatabases[dbName]
                        : null;
                    if (location !== expected)
                        throw new SnapshotEnvelopeError(`agent '${agentTypeName}' database '${dbName}' location '${location}' does not match snapshot location '${expected}'`);
                }
                // Hydrate only memory/temp databases; files were restored by the host.
                for (const part of decoded.databases) {
                    const handle = bound.databases.get(part.name);
                    sqliteExt.restoreDatabaseSync(handle, part.bytes);
                }
                for (const dbName of declared)
                    warmSnapshotDatabase(bound.databases.get(dbName), Object.hasOwn(decoded.fileDatabases, dbName), sqliteExt);
            }
        }
        else {
            if (decoded.kind !== "binary") {
                throw new SnapshotEnvelopeError(`agent '${agentTypeName}' expects a binary envelope but received ${snapshot.mimeType}`);
            }
            // The lifecycle strategy restored the custom state before methods were built.
        }
    };
    // Restore resources and database images before constructing shared methods.
    const constructorInput = await decodeConstructorInput(compiled, typedConstructor.value);
    const { scope, handlers, snapshot: bound, state, } = await initAgentInstance(agentTypeName, compiled, constructorInput, principal, {
        phantomId,
        parsedAgentId: agentIdString,
        saved,
        restoreDatabases,
    });
    activeAgent = {
        name: agentTypeName,
        scope,
        handlers,
        principal,
        snapshot: bound,
        state,
    };
};

const value = [3, 5, 7].reduce((sum, item) => sum * 2 + item, 0);
defineAgent({
    name: "JsGuestSmoke",
    description: `Effect initialized value: ${value}, clock: ${Date.now()}`,
    id: {},
    methods: { get: method({ input: {}, success: Schema.Number }) },
}).implement({
    init: () => Effect.succeed({}),
    methods: () => ({ get: () => Effect.succeed(value) }),
});

/**
 * Implementations of the Golem `agent-guest` host exports. Re-exported
 * from {@link ./index} as the package's public surface so that the
 * generated WIT bindings can find them at the package entry point.
 *
 * Users of this library define their agents/methods declaratively via
 * `defineAgent` + `defineMethod`, and `registerAgent` wires them into
 * these dispatchers — they should never need to interact with `guest`,
 * `saveSnapshot`, or `loadSnapshot` directly.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
const guest = {
    initialize: dispatchInitialize,
    invoke: dispatchInvoke,
    discoverAgentTypes: dispatchDiscoverAgentTypes,
    getDefinition: dispatchGetDefinition,
};
/**
 * Snapshotting hooks. Wired through to the dispatchers in `./agent`.
 * Agents that don't declare a `snapshot` field map to
 * `snapshotting: { tag: "disabled" }`, so the host should never invoke
 * these for them; the dispatchers raise a clear error if it does.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
const saveSnapshot = {
    save: dispatchSaveSnapshot,
};
/**
 * Snapshot-restore hook. Wired through to {@link dispatchLoadSnapshot}
 * in `./agent`. The host calls `load` instead of `initialize` when an
 * agent is being restored from a snapshot envelope.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
const loadSnapshot = {
    load: dispatchLoadSnapshot,
};

const golemTool010Guest = {
    discoverTools: () => [],
    getTool: (name) => {
        throw { tag: "invalid-tool-name", val: name };
    },
    invoke: async (name) => {
        throw { tag: "invalid-tool-name", val: name };
    },
};
const toolMiddlewareGuest = {
    discoverToolMiddlewares: () => [],
    getToolMiddleware: (name) => {
        throw { tag: "invalid-tool-name", val: name };
    },
    invokeToolMiddleware: async (name) => {
        throw { tag: "invalid-tool-name", val: name };
    },
};

export { guest as golemAgent200Guest, golemTool010Guest, loadSnapshot, saveSnapshot, toolMiddlewareGuest };
//# sourceMappingURL=smoke.mjs.map
