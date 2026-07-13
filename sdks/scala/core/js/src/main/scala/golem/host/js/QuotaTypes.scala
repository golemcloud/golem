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

package golem.host.js

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName

// ---------------------------------------------------------------------------
// golem:quota/types@1.5.0  –  JS facade traits
//
// The quota-token capability itself is the opaque `quota-token` resource from
// `golem:core/types@2.0.0`; the quota interface exposes only free functions and
// the `reservation` resource that operate on a handle. There is no longer a
// guest-visible record representation of a token.
// ---------------------------------------------------------------------------

// --- failed-reservation record ---

@js.native
sealed trait JsFailedReservation extends js.Object {
  @JSName("estimatedWaitNanos") def estimatedWaitNanos: js.UndefOr[js.BigInt] = js.native
}

// --- reservation resource ---
// Opaque host resource. `commit` is a static free function (see QuotaApi).

@js.native
sealed trait JsReservation extends js.Object

// --- quota-token resource ---
// Opaque, unforgeable host resource defined in `golem:core/types@2.0.0`. Guest
// code can only hold and move the handle; it has no readable structure.

@js.native
sealed trait JsQuotaTokenResource extends js.Object
