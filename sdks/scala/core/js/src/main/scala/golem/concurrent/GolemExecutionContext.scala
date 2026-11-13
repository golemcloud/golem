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

import scala.annotation.nowarn
import scala.concurrent.{ExecutionContext, ExecutionContextExecutor}
import scala.scalajs.concurrent.JSExecutionContext

/**
 * An execution context that runs each task in the custom durability scope of
 * the code that submitted it.
 *
 * A `Future` callback captures the scope when it is registered (`map`,
 * `flatMap`, `onComplete` and the like), not when its source completes, so a
 * continuation never runs in the scope of the sibling that completed its
 * source. A task given directly to `execute` captures the scope of the
 * submitter. The previous scope is restored after each task, also when it
 * throws.
 *
 * Use [[GolemExecutionContext.global]] (or
 * `import golem.concurrent.Implicits.global`) in agent code, and
 * [[GolemExecutionContext.wrap]] for an execution context of your own.
 * Callbacks run by other execution contexts, including
 * `ExecutionContext.parasitic`, run in whatever scope is current when their
 * source completes.
 */
final class GolemExecutionContext private (
  underlying: ExecutionContext,
  captured: CustomInvocationToken | Null
) extends ExecutionContextExecutor {

  @nowarn("cat=deprecation")
  override def prepare(): ExecutionContext =
    new GolemExecutionContext(underlying, CustomInvocationContext.capture())

  override def execute(runnable: Runnable): Unit = {
    val token = if (captured == null) CustomInvocationContext.capture() else captured
    underlying.execute { () =>
      CustomInvocationContext.run[Unit](token, () => runnable.run())
    }
  }

  override def reportFailure(cause: Throwable): Unit =
    underlying.reportFailure(cause)
}

object GolemExecutionContext {

  /** Runs tasks on the JavaScript job queue in the submitter's scope. */
  val global: ExecutionContextExecutor = wrap(JSExecutionContext.queue)

  /** Runs tasks on `underlying` in the submitter's scope. */
  def wrap(underlying: ExecutionContext): ExecutionContextExecutor =
    underlying match {
      case scoped: GolemExecutionContext => scoped
      case other                         => new GolemExecutionContext(other, null)
    }
}

object Implicits {
  implicit val global: ExecutionContextExecutor = GolemExecutionContext.global
}
