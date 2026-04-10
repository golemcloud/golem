/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.host.js

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName

// ---------------------------------------------------------------------------
// golem:quota/host@1.5.0  –  JS facade traits
// ---------------------------------------------------------------------------

// --- failed-reservation record ---

@js.native
sealed trait JsFailedReservation extends js.Object {
  @JSName("estimatedWaitNanos") def estimatedWaitNanos: js.UndefOr[js.BigInt] = js.native
}

// --- reservation resource ---

@js.native
sealed trait JsReservation extends js.Object {
  def commit(used: js.BigInt): Unit = js.native
}

// --- quota-token resource ---
// The WIT constructor maps to a JS class; methods are instance methods.

@js.native
sealed trait JsQuotaToken extends js.Object {
  def reserve(amount: js.BigInt): JsReservation             = js.native
  def split(childExpectedUse: js.BigInt): JsQuotaToken      = js.native
  def merge(other: JsQuotaToken): Unit                      = js.native
}
