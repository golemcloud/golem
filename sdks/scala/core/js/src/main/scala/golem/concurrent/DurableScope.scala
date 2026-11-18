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

import scala.concurrent.{Future, Promise}
import scala.scalajs.concurrent.JSExecutionContext
import scala.scalajs.js
import scala.scalajs.js.annotation.JSGlobal

/**
 * The scope of one live asynchronous custom operation.
 *
 * A scope admits nested operations until it is sealed. It is sealed when its
 * body has finished, no nested operation is open and no task that a
 * [[GolemExecutionContext]] submitted in the scope waits to run. A continuation
 * that is already ready can therefore still start a nested operation that the
 * scope waits for. An operation started after the seal belongs to the nearest
 * enclosing scope that still admits operations.
 */
private[golem] final class DurableScope private (
  val token: CustomInvocationToken,
  val outer: CustomInvocationToken,
  val parent: DurableScope | Null
) {
  private var openChildren = 0
  private var readyTasks   = 0
  private var sealedScope  = false
  private var closedScope  = false
  private var idle         = Option.empty[Promise[Unit]]

  def childStarted(): Unit = openChildren += 1

  def childClosed(): Unit = {
    openChildren -= 1
    scheduleCheck()
  }

  /**
   * Completes when the scope is sealed. Call it once, after the body finished.
   */
  def sealWhenIdle(): Future[Unit] = {
    val done = Promise[Unit]()
    idle = Some(done)
    scheduleCheck()
    done.future
  }

  def close(): Unit = closedScope = true

  private[concurrent] def taskSubmitted(): Unit = readyTasks += 1

  // A submitted task started, or its executor rejected it.
  private[concurrent] def taskReleased(): Unit = {
    readyTasks -= 1
    if (readyTasks == 0) scheduleCheck()
  }

  // Deferred, so that work the current task triggers after this call, such as
  // the callbacks of a future it completes, is counted first.
  private def scheduleCheck(): Unit =
    if (idle.isDefined) JSExecutionContext.queue.execute(() => check())

  private def check(): Unit =
    if (!sealedScope && openChildren == 0 && readyTasks == 0)
      idle.foreach { done =>
        sealedScope = true
        done.trySuccess(())
      }

  private def admits: Boolean = !sealedScope && !closedScope
}

private[golem] object DurableScope {
  private val scopes = new WeakMap[CustomInvocationToken, DurableScope]()

  /** Registers the scope of `token`, opened while `outer` was current. */
  def open(token: CustomInvocationToken, outer: CustomInvocationToken, parent: DurableScope | Null): DurableScope = {
    val scope = new DurableScope(token, outer, parent)
    scopes.set(token, scope)
    scope
  }

  /**
   * The scope that an operation started in `current` belongs to, with the token
   * to begin it in: the scope of `current`, else its nearest enclosing scope
   * that still admits operations, else the root.
   */
  def admitting(current: CustomInvocationToken): (DurableScope | Null, CustomInvocationToken) = {
    var token = current
    var scope = lookup(current)
    while (scope != null && !scope.admits) {
      token = scope.outer
      scope = scope.parent
    }
    (scope, token)
  }

  /** The scope whose seal waits for a task submitted in `token`. */
  private[concurrent] def waitingFor(token: CustomInvocationToken): DurableScope | Null = {
    var scope = lookup(token)
    while (scope != null && scope.closedScope) scope = scope.parent
    scope
  }

  private def lookup(token: CustomInvocationToken): DurableScope | Null =
    scopes.get(token).getOrElse(null)
}

@js.native
@JSGlobal("WeakMap")
private final class WeakMap[K <: js.Any, V] extends js.Object {
  def get(key: K): js.UndefOr[V]  = js.native
  def set(key: K, value: V): Unit = js.native
}
