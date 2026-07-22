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
 * Target Golem server for the generated bridge client, mirroring the Rust
 * bridge's `GolemServer`. Each variant resolves to a base worker-service URL
 * and an authentication token.
 */
sealed trait GolemServer extends Product with Serializable {

  /** Base URL of the worker service, without a trailing slash. */
  def url: String

  /** Bearer token used for the `Authorization` header. */
  def token: String
}

object GolemServer {
  private val LocalWellKnownToken = "5c832d93-ff85-4a8f-9803-513950fdfdb1"

  /** Local single-executable Golem server on `http://localhost:9881`. */
  case object Local extends GolemServer {
    val url: String   = "http://localhost:9881"
    val token: String = LocalWellKnownToken
  }

  /** Golem Cloud (release region). */
  final case class Cloud(token: String) extends GolemServer {
    val url: String = "https://release.api.golem.cloud"
  }

  /** A custom worker-service deployment. */
  final case class Custom(url: String, token: String) extends GolemServer
}
