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

package golem.host.js

import golem.{Principal, Uuid}
import zio.blocks.schema.json.Json

import scala.scalajs.js

object PrincipalConverter {

  def fromJs(dynamic: js.Dynamic): Principal = {
    val tag = dynamic.tag
    if (js.isUndefined(tag) || tag == null) Principal.Anonymous
    else
      tag.asInstanceOf[String] match {
        case "oidc" =>
          val p = dynamic.asInstanceOf[JsPrincipalOidc].value
          Principal.Oidc(
            sub = p.sub,
            issuer = p.issuer,
            claims = p.claims,
            email = p.email.toOption,
            name = p.name.toOption,
            emailVerified = p.emailVerified.toOption,
            givenName = p.givenName.toOption,
            familyName = p.familyName.toOption,
            picture = p.picture.toOption,
            preferredUsername = p.preferredUsername.toOption
          )
        case "agent" =>
          val p           = dynamic.asInstanceOf[JsPrincipalAgent].value
          val jsUuid      = p.agentId.componentId.uuid
          val componentId = Uuid(
            highBits = BigInt(jsUuid.highBits.toString),
            lowBits = BigInt(jsUuid.lowBits.toString)
          )
          Principal.Agent(
            componentId = componentId,
            agentId = p.agentId.agentId
          )
        case "golem-user" =>
          val p         = dynamic.asInstanceOf[JsPrincipalGolemUser].value
          val jsUuid    = p.accountId.uuid
          val accountId = Uuid(
            highBits = BigInt(jsUuid.highBits.toString),
            lowBits = BigInt(jsUuid.lowBits.toString)
          )
          Principal.GolemUser(accountId = accountId)
        case "anonymous" =>
          Principal.Anonymous
        case _ =>
          Principal.Anonymous
      }
  }

  def toJson(principal: Principal): Array[Byte] =
    principalToJson(principal).printBytes

  def fromJson(bytes: Array[Byte]): Either[String, Principal] =
    Json.parse(bytes).left.map(_.toString).flatMap(principalFromJson)

  private def principalToJson(principal: Principal): Json = principal match {
    case Principal.Anonymous =>
      Json.Object("tag" -> Json.String("anonymous"))

    case Principal.Agent(componentId, agentId) =>
      Json.Object(
        "tag" -> Json.String("agent"),
        "val" -> Json.Object(
          "componentId" -> Json.String(Uuid.toStandardString(componentId)),
          "agentId"     -> Json.String(agentId)
        )
      )

    case Principal.GolemUser(accountId) =>
      Json.Object(
        "tag" -> Json.String("golem-user"),
        "val" -> Json.Object(
          "accountId" -> Json.String(Uuid.toStandardString(accountId))
        )
      )

    case Principal.Oidc(sub, issuer, claims, email, name, emailVerified, givenName, familyName, picture, preferredUsername) =>
      Json.Object(
        "tag" -> Json.String("oidc"),
        "val" -> Json.Object(
          "sub"               -> Json.String(sub),
          "issuer"            -> Json.String(issuer),
          "email"             -> optString(email),
          "name"              -> optString(name),
          "emailVerified"     -> optBoolean(emailVerified),
          "givenName"         -> optString(givenName),
          "familyName"        -> optString(familyName),
          "picture"           -> optString(picture),
          "preferredUsername"  -> optString(preferredUsername),
          "claims"            -> Json.String(claims)
        )
      )
  }

  private def principalFromJson(json: Json): Either[String, Principal] =
    for {
      tag <- getStr(json, "tag")
      result <- tag match {
        case "anonymous" => Right(Principal.Anonymous)
        case "agent" =>
          for {
            valObj      <- getObj(json, "val")
            cidStr      <- getStr(valObj, "componentId")
            componentId <- Uuid.fromStandardString(cidStr)
            agentId     <- getStr(valObj, "agentId")
          } yield Principal.Agent(componentId, agentId)
        case "golem-user" =>
          for {
            valObj    <- getObj(json, "val")
            aidStr    <- getStr(valObj, "accountId")
            accountId <- Uuid.fromStandardString(aidStr)
          } yield Principal.GolemUser(accountId)
        case "oidc" =>
          for {
            valObj <- getObj(json, "val")
            sub    <- getStr(valObj, "sub")
            issuer <- getStr(valObj, "issuer")
            claims <- getStr(valObj, "claims")
          } yield Principal.Oidc(
            sub = sub,
            issuer = issuer,
            claims = claims,
            email = getOptStr(valObj, "email"),
            name = getOptStr(valObj, "name"),
            emailVerified = getOptBool(valObj, "emailVerified"),
            givenName = getOptStr(valObj, "givenName"),
            familyName = getOptStr(valObj, "familyName"),
            picture = getOptStr(valObj, "picture"),
            preferredUsername = getOptStr(valObj, "preferredUsername")
          )
        case other => Left(s"Unknown principal tag: $other")
      }
    } yield result

  private def optString(o: Option[String]): Json = o match {
    case Some(v) => Json.String(v)
    case None    => Json.Null
  }

  private def optBoolean(o: Option[Boolean]): Json = o match {
    case Some(v) => Json.Boolean(v)
    case None    => Json.Null
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

  private def getOptStr(json: Json, field: String): Option[String] =
    json.get(field).one.toOption.flatMap {
      case s: Json.String => Some(s.value)
      case _              => None
    }

  private def getOptBool(json: Json, field: String): Option[Boolean] =
    json.get(field).one.toOption.flatMap {
      case b: Json.Boolean => Some(b.value)
      case _               => None
    }
}
