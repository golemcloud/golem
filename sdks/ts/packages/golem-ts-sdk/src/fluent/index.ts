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

// Experimental fluent / config-object agent authoring surface (issue #3449),
// built on Standard Schema. Temporary and unstable: it will eventually replace
// the `@agent()` decorator surface.

// Register the built-in schema walkers as module-load side effects. The vendor
// walkers duck-type the schema's runtime structure and never import the vendor
// library, so none of zod/valibot/arktype/effect is baked into the WASM bundle.
import './schema/zod';
import './schema/valibot';
import './schema/arktype';
import './schema/effect';

export { defineAgent } from './defineAgent';
export type {
  AgentDefinition,
  AgentImpl,
  AgentImplementation,
  AgentSpec,
  ConfigSpec,
  ConfigView,
  FluentAgentThis,
  IdRecord,
  InitContext,
  MethodsRecord,
} from './defineAgent';

// Lazy, log-safe secret config handle.
export { Secret } from './secret';

export { method } from './method';
export type { InputRecord, MethodSpec } from './method';

export type { StandardSchemaV1 } from './schema/standardSchema';
export { s } from './schema/markers';
export { registerSchemaWalker, registeredVendors, compileSchema } from './schema/adapter';
export type { FluentCodec, SchemaWalker } from './schema/codec';

// wasm-RPC client for calling other agents.
export { clientFor, RemoteCallError } from './client';
export type {
  PhantomClientDetails,
  RemoteCallOptions,
  RemoteClient,
  RemoteClientFactory,
} from './client';

// Typed host surfaces (plain async).
export * from './keyvalue';
export * from './blobstore';
export * from './websocket';
export * from './rdbms';

// HTTP mount + endpoint declaration helpers.
export * as http from './http';
