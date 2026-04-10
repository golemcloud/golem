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

package golem

import zio.blocks.schema.Schema

sealed trait Principal extends Product with Serializable

object Principal {
  final case class Oidc(
    sub: String,
    issuer: String,
    claims: String,
    email: Option[String] = None,
    name: Option[String] = None,
    emailVerified: Option[Boolean] = None,
    givenName: Option[String] = None,
    familyName: Option[String] = None,
    picture: Option[String] = None,
    preferredUsername: Option[String] = None
  ) extends Principal

  final case class Agent(
    componentId: Uuid,
    agentId: String
  ) extends Principal

  final case class GolemUser(
    accountId: Uuid
  ) extends Principal

  case object Anonymous extends Principal

  implicit val schema: Schema[Principal] = Schema.derived
}
