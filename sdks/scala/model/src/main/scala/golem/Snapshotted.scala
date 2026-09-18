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

package golem

/**
 * Mixin trait for automatic JSON-based snapshotting.
 *
 * Mix this into your agent implementation class to get automatic snapshot
 * save/load support. Bundle all mutable state into a case class `S` with a
 * `zio.blocks.schema.Schema[S]` instance and provide it as `var state`.
 *
 * The macro detects this trait on the implementation class and generates
 * snapshot handlers that serialize/deserialize `state` as JSON using the state
 * type's schema. The implementation companion must construct the fresh
 * implementation from decoded state.
 *
 * ==Example==
 * {{{
 * import zio.blocks.schema.Schema
 *
 * case class CounterState(value: Int) derives Schema
 *
 * @agentDefinition(snapshotting = "enabled")
 * trait MyCounter extends BaseAgent {
 *   class Id(val value: String)
 *   def increment(): Future[Int]
 * }
 *
 * @agentImplementation()
 * final class MyCounterImpl(name: String)
 *   extends MyCounter with Snapshotted[CounterState] {
 *
 *   var state: CounterState = CounterState(0)
 *   override def increment(): Future[Int] = Future.successful {
 *     state = state.copy(value = state.value + 1)
 *     state.value
 *   }
 * }
 *
 * object MyCounterImpl {
 *   def loadSnapshot(state: CounterState, context: SnapshotRestoreContext): Future[MyCounterImpl] =
 *     Future.successful(new MyCounterImpl(state))
 * }
 * }}}
 *
 * @tparam S
 *   The state type. A `zio.blocks.schema.Schema[S]` must be available at the
 *   implementation declaration site.
 */
trait Snapshotted[S] {
  var state: S
}
