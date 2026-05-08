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

package golem.runtime.rpc

import golem.Datetime
import golem.runtime.AgentMethod

import scala.concurrent.Future

/**
 * Base class for generated per-method RPC wrappers.
 *
 * Centralises the lazy method lookup by name, plus typed delegation to
 * [[AgentClientRuntime.ResolvedAgent]]'s `await`, `trigger`, and `schedule`
 * methods. Generated subclasses only need to provide typed `apply`, `trigger`,
 * and `scheduleAt` methods that pack their parameters and delegate to the
 * `protected` helpers below.
 */
abstract class AbstractRemoteMethod[Trait, In, Out] protected (
  resolved: AgentClientRuntime.ResolvedAgent[Trait],
  methodName: String
) {
  protected final lazy val method: AgentMethod[Trait, In, Out] =
    resolved.methodByName[In, Out](methodName)

  protected final def awaitWith(input: In): Future[Out] =
    resolved.await(method, input)

  protected final def cancelableAwaitWith(input: In): (Future[Out], CancellationToken) =
    resolved.cancelableAwait(method, input)

  protected final def triggerWith(input: In): Future[Unit] =
    resolved.trigger(method, input)

  protected final def scheduleWith(input: In, when: Datetime): Future[Unit] =
    resolved.schedule(method, when, input)

  protected final def scheduleCancelableWith(input: In, when: Datetime): Future[CancellationToken] =
    resolved.scheduleCancelable(method, when, input)
}
