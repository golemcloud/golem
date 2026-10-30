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

package golem.bridge.runtime

/**
 * Cancels the streaming call it was returned with. Before the server accepted
 * the call, cancelling stops its reconnect attempts; after it, cancelling
 * closes the session. The call fails with a [[BridgeCancelledException]].
 */
final class CancellationToken private[runtime] (private val cancelFn: () => Unit) {
  def cancel(): Unit = cancelFn()
}
