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

import golem.HostApi
import golem.HostApi.ForkResult
import golem.runtime.annotations.agentImplementation
import zio.blocks.schema.Schema

import scala.annotation.unused
import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

final case class ForkState(count: Int)

object ForkState {
  implicit val schema: Schema[ForkState] = Schema.derived
}

@agentImplementation()
final class ForkDemoImpl(@unused private val name: String) extends ForkDemo {

  override def runFork(): Future[String] = {
    val promiseId = HostApi.createPromise()

    HostApi.fork() match {
      case ForkResult.Original(_) =>
        HostApi.awaitPromise(promiseId).map { result =>
          val msg = new String(result, "UTF-8")
          s"original-joined: $msg"
        }

      case ForkResult.Forked(_) =>
        val msg = "Hello from forked agent!"
        HostApi.completePromise(promiseId, msg.getBytes("UTF-8"))
        Future.successful(s"forked result")
    }
  }

  override def runForkJson(): Future[String] = {
    val promiseId = HostApi.createPromise()

    HostApi.fork() match {
      case ForkResult.Original(_) =>
        HostApi.awaitPromiseJson[ForkState](promiseId).map { state =>
          s"original-joined-json: count=${state.count}"
        }

      case ForkResult.Forked(_) =>
        val completed = HostApi.completePromiseJson(promiseId, ForkState(count = 42))
        Future.successful(s"forked-completed: $completed")
    }
  }
}
