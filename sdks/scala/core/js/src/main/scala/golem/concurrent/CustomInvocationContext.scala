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

package golem.concurrent

import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

/**
 * An opaque runtime token that names a custom durability scope, or the
 * invocation's root.
 */
@js.native
private[golem] sealed trait CustomInvocationToken extends js.Object

/**
 * Facade for the runtime-owned custom invocation context cell of
 * `wasm-rquickjs:custom-invocation-context`.
 *
 * The runtime carries the current token across promise reactions, native await,
 * timers and async host calls, and keeps one host snapshot per open scope. A
 * token of a closed scope resolves to its nearest open ancestor, or to the
 * root, so installing a captured token never traps the invocation.
 */
@js.native
@JSImport("wasm-rquickjs:custom-invocation-context", JSImport.Namespace)
private[golem] object CustomInvocationContext extends js.Object {

  /** The current token. Makes no host call at the root. */
  def capture(): CustomInvocationToken = js.native

  /**
   * Runs `fn` with `token` installed and restores the previous token
   * afterwards.
   */
  def run[A](token: CustomInvocationToken, fn: js.Function0[A]): A = js.native

  /**
   * Enters a live invocation admitted asynchronously and makes its scope
   * current.
   */
  def enterLive(live: js.Any): CustomInvocationToken = js.native

  /** Closes the scope of `token`. Throws if the scope has open descendants. */
  def close(token: CustomInvocationToken): Unit = js.native
}
