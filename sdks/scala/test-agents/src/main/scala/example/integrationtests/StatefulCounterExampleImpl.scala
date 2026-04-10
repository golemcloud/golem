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

package example.integrationtests

import golem.runtime.annotations.agentImplementation

import scala.concurrent.Future

@agentImplementation()
final class StatefulCounterImpl(private val initialCount: Int) extends StatefulCounter {
  private var count: Int = initialCount

  override def increment(): Future[Int] =
    Future.successful {
      count += 1
      count
    }

  override def current(): Future[Int] =
    Future.successful(count)
}

@agentImplementation()
final class StatefulCallerImpl(private val initialCount: Int) extends StatefulCaller {
  private val counter = StatefulCounterClient.get(initialCount)

  override def remoteIncrement(): Future[Int] =
    counter.increment()
}
