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

package golem.tool

/**
 * Opaque handle to the byte stream supplied as a tool invocation's stdin. A
 * tool method parameter of this type is auto-injected from the invocation and
 * excluded from the tool's input schema. The platform layer (Scala.js guest)
 * provides the concrete implementation carrying the underlying WASI stream.
 */
trait ToolInputStream

/**
 * Opaque handle to the process stdout stream a tool invocation may write to. A
 * tool method parameter of this type is auto-injected and excluded from the
 * tool's input schema; when present, the invocation result carries the stream
 * back to the caller.
 */
trait ToolOutputStream
