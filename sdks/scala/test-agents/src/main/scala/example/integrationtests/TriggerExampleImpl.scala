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

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class TriggerTargetImpl(@unused private val name: String) extends TriggerTarget {
  private var count: Int = 0

  override def process(x: Int, label: String): Future[Int] =
    Future.successful {
      count += x
      count
    }

  override def ping(): Future[String] =
    Future.successful("pong")
}

@agentImplementation()
final class TriggerCallerImpl(@unused private val name: String) extends TriggerCaller {
  private val target = TriggerTargetClient.get("target-instance")

  override def fireProcess(): Future[String] = {
    target.process.trigger(42, "hello")
    Future.successful("triggered process(42, \"hello\") on target-instance")
  }

  override def firePing(): Future[String] = {
    target.ping.trigger()
    Future.successful("triggered ping() on target-instance")
  }
}
