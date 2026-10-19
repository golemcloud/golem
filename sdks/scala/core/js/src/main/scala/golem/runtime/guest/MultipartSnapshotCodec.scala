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

package golem.runtime.guest

import golem.Principal
import golem.host.js.PrincipalConverter
import golem.runtime.{MultipartSnapshot, SnapshotError, SnapshotPart, SnapshotPayload}
import zio.blocks.schema.json.Json

private[runtime] object MultipartSnapshotCodec {
  private def checked[A](value: Either[?, A]): A                = value.fold(error => throw SnapshotError(error.toString), identity)
  private def ensure(condition: Boolean, message: String): Unit =
    if (!condition) throw SnapshotError(message)

  private def at(data: Array[Byte], value: Array[Byte], offset: Int): Boolean =
    offset >= 0 && offset + value.length <= data.length && value.indices.forall(i => data(offset + i) == value(i))

  private def delimiter(
    data: Array[Byte],
    marker: Array[Byte],
    newline: Array[Byte],
    offset: Int
  ): Option[(Int, Boolean)] =
    if (!at(data, marker, offset)) None
    else {
      val end    = offset + marker.length
      val close  = at(data, Array[Byte](45, 45), end)
      val suffix = end + (if (close) 2 else 0)
      if (at(data, newline, suffix)) Some((suffix + newline.length, close))
      else if (close && suffix == data.length) Some((suffix, close))
      else None
    }

  private def boundary(mime: String): String = {
    val fields = mime.split(";", -1)
    ensure(fields.length == 2 && fields(0).trim.equalsIgnoreCase("multipart/mixed"), "Invalid multipart MIME")
    val param = fields(1).split("=", 2)
    ensure(param.length == 2 && param(0).trim.equalsIgnoreCase("boundary"), "Missing multipart boundary")
    val raw   = param(1).trim
    val value = if (raw.startsWith("\"")) {
      ensure(raw.length >= 2 && raw.endsWith("\""), "Invalid boundary quoting")
      raw.substring(1, raw.length - 1)
    } else {
      ensure(raw.matches("[A-Za-z0-9'+_.-]+"), "Invalid unquoted boundary")
      raw
    }
    ensure(value.length <= 70 && value.matches("[A-Za-z0-9'()+_,./:=?-]+"), "Invalid boundary")
    value
  }

  private[runtime] def parseParts(data: Array[Byte], mime: String): Either[String, Vector[(String, SnapshotPart)]] =
    scala.util.Try {
      val marker  = ("--" + boundary(mime)).getBytes("US-ASCII")
      val start   = if (at(data, Array[Byte](13, 10), 0)) 2 else if (at(data, Array[Byte](10), 0)) 1 else 0
      val end     = start + marker.length
      val suffix  = end + (if (at(data, Array[Byte](45, 45), end)) 2 else 0)
      val newline =
        if (at(data, Array[Byte](13, 10), suffix) || (suffix == data.length && start == 2))
          Array[Byte](13, 10)
        else Array[Byte](10)
      ensure(start == 0 || start == newline.length, "Invalid leading newline")
      var (pos, closing) =
        delimiter(data, marker, newline, start).getOrElse(throw SnapshotError("Invalid first delimiter"))
      val result = Vector.newBuilder[(String, SnapshotPart)]
      val names  = scala.collection.mutable.HashSet.empty[String]
      while (!closing) {
        val headers = scala.collection.mutable.Map.empty[String, String]
        var done    = false
        while (!done) {
          val end = data.indexOf(10.toByte, pos)
          ensure(end >= 0, "Truncated headers")
          val bytes = data.slice(pos, if (end > pos && data(end - 1) == 13) end - 1 else end)
          pos = end + 1
          if (bytes.isEmpty) done = true
          else {
            ensure(bytes.forall(b => b >= 32 && b <= 126) && bytes(0) != 32, "Invalid header")
            val line  = new String(bytes, "US-ASCII")
            val colon = line.indexOf(':')
            ensure(colon > 0, "Invalid header")
            val key = line.substring(0, colon).toLowerCase(java.util.Locale.ROOT)
            ensure(
              !headers.contains(key) && Set("content-type", "content-disposition")(key),
              "Duplicate or unknown header"
            )
            headers(key) = line.substring(colon + 1).trim
          }
        }
        ensure(headers.size == 2, "Missing structural header")
        val disposition = headers("content-disposition")
        val pattern     = "(?i)attachment;\\s*name=\"([^\"\\\\]+)\"".r
        val name        = disposition match {
          case pattern(value) => value
          case _              => throw SnapshotError("Invalid disposition")
        }
        ensure(names.add(name), "Duplicate snapshot part")
        val next = (pos until data.length).iterator.flatMap { i =>
          if (at(data, newline, i)) delimiter(data, marker, newline, i + newline.length).map(d => (i, d))
          else None
        }.nextOption().getOrElse(throw SnapshotError("Missing closing delimiter"))
        result += name -> SnapshotPart(data.slice(pos, next._1), headers("content-type"))
        pos = next._2._1
        closing = next._2._2
      }
      ensure(pos == data.length, "Multipart epilogue")
      result.result()
    }.toEither.left.map(_.getMessage)

  def encode(snapshot: MultipartSnapshot, principal: Principal): Either[String, SnapshotPayload] =
    scala.util.Try {
      val principalJson = new String(PrincipalConverter.toJson(principal), "UTF-8")
      val envelope      = s"""{"version":1,"principal":$principalJson,"state":${snapshot.state.print}}""".getBytes("UTF-8")
      val parts         = Vector("state" -> SnapshotPart(envelope, "application/json")) ++ snapshot.parts.toVector.map {
        case (name, part) =>
          ensure(MultipartSnapshot.validName(name), s"Invalid snapshot part '$name'")
          ("part:" + name) -> part.copy(contentType = checked(MultipartSnapshot.normalizeContentType(part.contentType)))
      }
      val newline = Array[Byte](13, 10)
      val b       = Iterator
        .from(0)
        .map(n => s"golem-snapshot-$n")
        .find { value =>
          val marker = ("--" + value).getBytes("US-ASCII")
          !parts.exists { case (_, part) =>
            val framed = part.bytes ++ newline
            delimiter(framed, marker, newline, 0).isDefined || framed.indices.exists(i =>
              at(framed, newline, i) && delimiter(framed, marker, newline, i + 2).isDefined
            )
          }
        }
        .get
      val out = scala.collection.mutable.ArrayBuffer.empty[Byte]
      parts.foreach { case (name, part) =>
        out ++= s"--$b\r\nContent-Disposition: attachment; name=\"$name\"\r\nContent-Type: ${part.contentType}\r\n\r\n"
          .getBytes("US-ASCII")
        out ++= part.bytes
        out ++= newline
      }
      out ++= s"--$b--\r\n".getBytes("US-ASCII")
      SnapshotPayload(out.toArray, s"multipart/mixed; boundary=$b")
    }.toEither.left.map(_.getMessage)

  private def objectFields(json: Json): Map[String, Json] = json match {
    case Json.Object(fields) =>
      ensure(fields.map(_._1).distinct.size == fields.size, "Duplicate metadata field")
      fields.iterator.toMap
    case _ => throw SnapshotError("Metadata must be an object")
  }

  // Json numbers are normalized by the AST; retain the top-level version token.
  private def versionToken(text: String): String = {
    var pos                = 1
    def whitespace(): Unit = while (pos < text.length && text.charAt(pos).isWhitespace) pos += 1
    def stringEnd(): Unit  = {
      pos += 1
      while (text.charAt(pos) != '"') {
        if (text.charAt(pos) == '\\') pos += 1
        pos += 1
      }
      pos += 1
    }
    while (pos < text.length) {
      whitespace()
      val keyStart = pos
      stringEnd()
      val key = checked(Json.parse(text.substring(keyStart, pos)))
      whitespace()
      pos += 1
      whitespace()
      val start    = pos
      var depth    = 0
      var complete = false
      while (!complete && pos < text.length) {
        text.charAt(pos) match {
          case '"'                     => stringEnd()
          case '[' | '{'               => depth += 1; pos += 1
          case ']' | '}' if depth > 0  => depth -= 1; pos += 1
          case ',' | '}' if depth == 0 => complete = true
          case _                       => pos += 1
        }
      }
      if (key == Json.String("version")) return text.substring(start, pos).trim
      pos += 1
    }
    throw SnapshotError("Missing version")
  }

  def decode(data: Array[Byte], mime: String): Either[String, (Principal, MultipartSnapshot)] =
    scala.util.Try {
      val parts     = checked(parseParts(data, mime))
      val statePart = parts.find(_._1 == "state").getOrElse(throw SnapshotError("Missing state"))._2
      ensure(statePart.contentType == "application/json", "Invalid state content type")
      val text = new String(statePart.bytes, "UTF-8")
      ensure(java.util.Arrays.equals(text.getBytes("UTF-8"), statePart.bytes), "Invalid JSON UTF-8")
      val envelope = objectFields(checked(Json.parse(statePart.bytes)))
      ensure(
        envelope.contains("version") && envelope.contains("principal") && envelope.contains("state"),
        "Incomplete envelope"
      )
      ensure(versionToken(text.trim) == "1", "Multipart version must be integer 1")
      val principal = envelope("principal")
      val metadata  = objectFields(principal)
      if (metadata.get("tag") != Some(Json.String("anonymous"))) {
        val value = objectFields(metadata.getOrElse("val", throw SnapshotError("Missing principal payload")))
        if (metadata.get("tag") == Some(Json.String("oidc"))) {
          Set("email", "name", "givenName", "familyName", "picture", "preferredUsername").foreach { key =>
            ensure(
              value.get(key).forall(v => v == Json.Null || v.isInstanceOf[Json.String]),
              "Invalid principal optional string"
            )
          }
          ensure(
            value.get("emailVerified").forall(v => v == Json.Null || v.isInstanceOf[Json.Boolean]),
            "Invalid principal optional boolean"
          )
        }
      }
      val recovered = checked(PrincipalConverter.fromJson(principal.printBytes))
      val userParts = parts
        .filterNot(_._1 == "state")
        .map { case (wireName, part) =>
          ensure(
            wireName.startsWith("part:") && MultipartSnapshot.validName(wireName.drop(5)),
            "Invalid part namespace/name"
          )
          wireName.drop(5) -> part.copy(contentType = checked(MultipartSnapshot.normalizeContentType(part.contentType)))
        }
        .toMap
      recovered -> MultipartSnapshot(envelope("state"), userParts)
    }.toEither.left.map(_.getMessage)
}
