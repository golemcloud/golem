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

package golem.host.js

import golem.CanonicalUuid
import zio.blocks.schema.json.Json

/**
 * Checks the principal of a 1.6 snapshot envelope, and discards it.
 *
 * Snapshots written by 1.6 carry the 1.6 principal. A 2.0 agent reads such a
 * snapshot with the same strictness as before (a malformed principal still
 * fails the load) and then drops the principal: the caller of an invocation
 * comes from the host, never from saved state.
 */
object LegacySnapshotPrincipal {

  /** Validates the JSON bytes of a 1.6 principal. */
  def check(bytes: Array[Byte]): Either[String, Unit] =
    Json.parse(bytes).left.map(_.toString).flatMap(check)

  /** Validates a 1.6 principal value. */
  def check(json: Json): Either[String, Unit] =
    getStr(json, "tag").flatMap {
      case "anonymous" => Right(())
      case "agent"     =>
        for {
          valObj <- getObj(json, "val")
          cidStr <- getStr(valObj, "componentId")
          _      <- CanonicalUuid.parse(cidStr)
          _      <- getStr(valObj, "agentId")
        } yield ()
      case "golem-user" =>
        for {
          valObj <- getObj(json, "val")
          aidStr <- getStr(valObj, "accountId")
          _      <- CanonicalUuid.parse(aidStr)
        } yield ()
      case "oidc" =>
        for {
          valObj <- getObj(json, "val")
          _      <- getStr(valObj, "sub")
          _      <- getStr(valObj, "issuer")
          _      <- getStr(valObj, "claims")
        } yield ()
      case other => Left(s"Unknown principal tag: $other")
    }

  private def getStr(json: Json, field: String): Either[String, String] =
    json.get(field).one.left.map(_.toString).flatMap {
      case s: Json.String => Right(s.value)
      case other          => Left(s"Expected string for '$field', got: ${other.print}")
    }

  private def getObj(json: Json, field: String): Either[String, Json] =
    json.get(field).one.left.map(_.toString).flatMap {
      case o: Json.Object => Right(o)
      case other          => Left(s"Expected object for '$field', got: ${other.print}")
    }
}
