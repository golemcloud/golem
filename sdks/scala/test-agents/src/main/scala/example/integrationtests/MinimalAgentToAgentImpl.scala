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
final class WorkerImpl(private val shardName: String, private val shardIndex: Int) extends Worker {
  override def reverse(input: String): Future[String] =
    Future.successful(s"$shardName:$shardIndex:" + input.reverse)

  override def handle(payload: TypedPayload): Future[TypedReply] =
    Future.successful(
      TypedReply(
        shardName = shardName,
        shardIndex = shardIndex,
        reversed = payload.name.reverse,
        payload = payload
      )
    )
}

@agentImplementation()
final class CoordinatorImpl(@unused id: String) extends Coordinator {
  override def route(shardName: String, shardIndex: Int, input: String): Future[String] = {
    val worker = WorkerClient.get(shardName, shardIndex)
    worker.reverse(input)
  }

  override def routeTyped(shardName: String, shardIndex: Int, payload: TypedPayload): Future[TypedReply] = {
    val worker = WorkerClient.get(shardName, shardIndex)
    worker.handle(payload)
  }
}
