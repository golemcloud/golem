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

import type { Principal as HostPrincipal } from 'golem:agent/common@2.0.0';
import type {
  InvocationResult,
  Tool,
  ToolError,
  ToolMiddleware,
  TypedSchemaValue,
  UnderlyingTool,
} from 'golem:tool/common@0.1.0';
import { sdkPrincipalFromHost } from '../../principal';
import {
  middlewareRegistrationError,
  ToolMiddlewareRegistry,
} from '../registry/toolMiddlewareRegistry';

export interface GolemToolMiddlewareGuest {
  discoverToolMiddlewares(): ToolMiddleware[];
  getToolMiddleware(name: string): ToolMiddleware;
  invokeToolMiddleware(
    middlewareName: string,
    toolName: string,
    toolMetadata: Tool,
    commandPath: string[],
    input: TypedSchemaValue,
    stdin: AsyncIterable<number> | undefined,
    principal: HostPrincipal,
    wrapped: Pick<UnderlyingTool, 'invoke'>,
  ): Promise<InvocationResult>;
}

function discoverToolMiddlewares(): ToolMiddleware[] {
  throwRegistrationError();
  return ToolMiddlewareRegistry.getRegisteredMiddlewares();
}

function getToolMiddleware(name: string): ToolMiddleware {
  throwRegistrationError();
  const entry = ToolMiddlewareRegistry.get(name);
  if (!entry) throw invalidToolName(name);
  return entry.encoded;
}

async function invokeToolMiddleware(
  middlewareName: string,
  toolName: string,
  toolMetadata: Tool,
  commandPath: string[],
  input: TypedSchemaValue,
  stdin: AsyncIterable<number> | undefined,
  principal: HostPrincipal,
  wrapped: Pick<UnderlyingTool, 'invoke'>,
): Promise<InvocationResult> {
  throwRegistrationError();
  const entry = ToolMiddlewareRegistry.get(middlewareName);
  if (!entry) throw invalidToolName(middlewareName);
  const invoke = entry.invoke;
  return await invoke({
    toolName,
    toolMetadata,
    commandPath,
    input,
    stdin,
    principal: sdkPrincipalFromHost(principal),
    wrapped,
  });
}

function throwRegistrationError(): void {
  const error = middlewareRegistrationError();
  if (error) throw error;
}

function invalidToolName(name: string): ToolError {
  return { tag: 'invalid-tool-name', val: name };
}

export const golemTool010ToolMiddlewareGuest: GolemToolMiddlewareGuest = {
  discoverToolMiddlewares,
  getToolMiddleware,
  invokeToolMiddleware,
};
