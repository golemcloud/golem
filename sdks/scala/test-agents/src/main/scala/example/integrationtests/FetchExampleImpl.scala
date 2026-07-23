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

package example.integrationtests

import golem.runtime.annotations.agentImplementation
import zio._
import zio.http._

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class FetchAgentImpl(@unused private val key: String) extends FetchAgent {

  override def fetchFromPort(port: Int): Future[String] = {
    val effect =
      (for {
        response <- ZIO.serviceWithZIO[Client] { client =>
                      client.url(url"http://localhost").port(port).batched.get("/test")
                    }
        body <- response.body.asString
      } yield body).provide(ZClient.default)

    Unsafe.unsafe { implicit u =>
      Runtime.default.unsafe.runToFuture(effect)
    }
  }
}
