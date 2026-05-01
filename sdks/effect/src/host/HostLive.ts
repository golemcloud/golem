/**
 * Top-level merge of every per-WIT-interface "Live" host layer. Built
 * once at module load (consumed by the agent dispatcher's
 * `provideUserRuntime` helper) and shared across every dispatch entry
 * (`initialize`, `invoke`, `save-snapshot`, `load-snapshot`).
 *
 * Each constituent layer is a thin wrapper around the same WIT
 * specifier import the SDK already used; production resolution is
 * unchanged. Tests substitute fakes via `Layer.succeed(Tag, fake)` /
 * `Effect.provide(eff, fakeLayer)` rather than the legacy
 * `__setX/__resetX` indirection.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Layer } from "effect"
import { WrapSemaphore, WrapSemaphoreLive } from "../DurableFunction.js"
import { AgentHostClient, AgentHostLive } from "./AgentHostClient.js"
import { BlobstoreClient, BlobstoreLive } from "./BlobstoreClient.js"
import { ConfigClient, ConfigLive } from "./ConfigClient.js"
import { DurabilityClient, DurabilityLive } from "./DurabilityClient.js"
import { DurabilityModeClient, DurabilityModeLive } from "./DurabilityModeClient.js"
import { EnvironmentClient, EnvironmentLive } from "./EnvironmentClient.js"
import { IgniteHostClient, IgniteHostLive } from "./IgniteHostClient.js"
import { KeyValueClient, KeyValueLive } from "./KeyValueClient.js"
import { LoggingHost, LoggingHostLive } from "./LoggingHost.js"
import { MysqlHostClient, MysqlHostLive } from "./MysqlHostClient.js"
import { NodeSqliteClient, NodeSqliteLive } from "./NodeSqliteClient.js"
import { OplogClient, OplogLive } from "./OplogClient.js"
import { PostgresHostClient, PostgresHostLive } from "./PostgresHostClient.js"
import { PromiseClient, PromiseLive } from "./PromiseClient.js"
import { QuotaClient, QuotaLive } from "./QuotaClient.js"
import { RetryClient, RetryLive } from "./RetryClient.js"
import { RpcClient, RpcLive } from "./RpcClient.js"
import { SqliteHostExtClient, SqliteHostExtLive } from "./SqliteHostExtClient.js"
import { TracingHost, TracingHostLive } from "./TracingHost.js"
import { WebsocketClient, WebsocketLive } from "./WebsocketClient.js"

export {
  AgentHostClient,
  BlobstoreClient,
  ConfigClient,
  DurabilityClient,
  DurabilityModeClient,
  EnvironmentClient,
  IgniteHostClient,
  KeyValueClient,
  LoggingHost,
  MysqlHostClient,
  NodeSqliteClient,
  OplogClient,
  PostgresHostClient,
  PromiseClient,
  QuotaClient,
  RetryClient,
  RpcClient,
  SqliteHostExtClient,
  TracingHost,
  WebsocketClient,
}

/**
 * Union of every host-service tag the dispatcher provides to user
 * effects via {@link HostLive}. SDK signatures (`Handler`, `impl`)
 * widen their `R` channel with this union so user code can yield
 * from any host-service tag without leaking it into the user-visible
 * required-services slot.
 */
export type HostServices =
  | EnvironmentClient
  | ConfigClient
  | AgentHostClient
  | PromiseClient
  | DurabilityClient
  | DurabilityModeClient
  | OplogClient
  | WrapSemaphore
  | RpcClient
  | LoggingHost
  | TracingHost
  | QuotaClient
  | RetryClient
  | NodeSqliteClient
  | SqliteHostExtClient
  | PostgresHostClient
  | MysqlHostClient
  | IgniteHostClient
  | WebsocketClient
  | BlobstoreClient
  | KeyValueClient

export const HostLive: Layer.Layer<HostServices> = Layer.mergeAll(
  EnvironmentLive,
  ConfigLive,
  AgentHostLive,
  PromiseLive,
  DurabilityLive,
  DurabilityModeLive,
  OplogLive,
  WrapSemaphoreLive,
  RpcLive,
  LoggingHostLive,
  TracingHostLive,
  QuotaLive,
  RetryLive,
  NodeSqliteLive,
  SqliteHostExtLive,
  PostgresHostLive,
  MysqlHostLive,
  IgniteHostLive,
  WebsocketLive,
  BlobstoreLive,
  KeyValueLive,
)
