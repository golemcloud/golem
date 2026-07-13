/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.host

package object js {
  // golem:agent/common@2.0.0
  type JsAgentMode         = String // "durable" | "ephemeral"
  type JsAgentConfigSource = String // "local" | "secret"
  type JsSystemVariable    = String // "agent-type" | "agent-version"

  // golem:api/host@1.5.0
  type JsUpdateMode             = String // "automatic" | "snapshot-based"
  type JsAgentStatus            = String // "running" | "idle" | "suspended" | "interrupted" | "retrying" | "failed" | "exited"
  type JsFilterComparator       = String // "equal" | "not-equal" | "greater-equal" | "greater" | "less-equal" | "less"
  type JsStringFilterComparator = String // "equal" | "not-equal" | "like" | "not-like" | "starts-with"

  // DurabilityTypes
  type JsDurableFunctionType = JsWrappedFunctionType
  type JsOplogEntryVersion   = String // "v1" | "v2"
}
