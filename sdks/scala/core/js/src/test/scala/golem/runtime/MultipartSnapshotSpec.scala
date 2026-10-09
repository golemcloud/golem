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

package golem.runtime

import golem.{BaseAgent, FutureInterop, Principal}
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import golem.runtime.autowire.{AgentImplementation, SchemaPayload}
import golem.runtime.guest.{Guest, MultipartSnapshotCodec}
import golem.host.js.JsSnapshot
import zio.blocks.schema.Schema
import zio.blocks.schema.json.Json
import zio.test._
import zio.ZIO
import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport
import scala.scalajs.js.typedarray.Uint8Array

object MultipartSnapshotSpec extends ZIOSpecDefault {
  final case class Saved(revision: Int, label: String) derives Schema

  @agentDefinition("multipart-index", snapshotting = "enabled")
  trait Index extends BaseAgent {
    class Id(val name: String)
    def size(): Int
  }

  @agentImplementation()
  final class IndexImpl(name: String) extends Index {
    if (name == "init") IndexImpl.initializations += 1
    var saved: Saved                                   = Saved(0, "init")
    var index: Array[Byte]                             = Array.emptyByteArray
    def size(): Int                                    = index.length
    def saveSnapshotParts(): Future[MultipartSnapshot] = Future.fromTry(
      MultipartSnapshot.fromState(saved, Map("index" -> SnapshotPart(index.clone(), "application/octet-stream"))).toTry
    )
  }
  object IndexImpl {
    var initializations                                                                                    = 0
    var restorations                                                                                       = 0
    def loadSnapshotParts(snapshot: MultipartSnapshot, context: SnapshotRestoreContext): Future[IndexImpl] = {
      restorations += 1
      Future.fromTry((for {
        state <- snapshot.decodeState[Saved]
        bytes <- snapshot.requirePart("index", "Application/Octet-Stream")
      } yield {
        val restored = new IndexImpl("restored:" + context.identity[String](0))
        restored.saved = state
        restored.index = bytes.clone()
        restored
      }).toTry)
    }
  }
  private lazy val definition = AgentImplementation.registerClass[Index, IndexImpl]

  @js.native
  @JSImport("node:fs", "readFileSync")
  private def readFile(path: String, encoding: String): String = js.native

  private def wire(state: String, extra: String = ""): Array[Byte] =
    s"--b\r\nContent-Type: application/json\r\nContent-Disposition: attachment; name=\"state\"\r\n\r\n$state\r\n$extra--b--\r\n"
      .getBytes("UTF-8")
  private val envelope                                         = """{"version":1,"principal":{"tag":"anonymous"},"state":null}"""
  private def jsSnapshot(payload: SnapshotPayload): JsSnapshot = {
    val bytes = new Uint8Array(payload.bytes.length)
    payload.bytes.indices.foreach(i => bytes(i) = payload.bytes(i))
    JsSnapshot(bytes, payload.mimeType)
  }

  def spec = suite("MultipartSnapshotSpec")(
    test("shared framing fixtures preserve exact opaque bodies") {
      val fixtures = Json.parse(readFile("../../test-data/snapshot-multipart/framing.json", "utf8")).toOption.get
      val b        = fixtures.get("boundary").one.toOption.get.asInstanceOf[Json.String].value
      val cases    = fixtures.get("valid").one.toOption.get.asInstanceOf[Json.Array].value
      val results  = cases.map { fixture =>
        def str(key: String) = fixture.get(key).one.toOption.get.asInstanceOf[Json.String].value
        val nl               = str("newline")
        val data             =
          s"--$b${nl}Content-Disposition: attachment; name=\"part:opaque\"${nl}Content-Type: application/octet-stream$nl$nl${str("payload")}$nl--$b--$nl"
            .getBytes("UTF-8")
        val expected = str("hex").grouped(2).map(Integer.parseInt(_, 16).toByte).toVector
        MultipartSnapshotCodec
          .parseParts(data, s"multipart/mixed; boundary=$b")
          .exists(parts => parts.size == 1 && parts.head._2.bytes.toVector == expected)
      }
      assertTrue(results.forall(identity))
    },
    test("schema helpers, dynamic names, Unicode principal and binary bytes roundtrip") {
      val bytes = (0 to 255).map(_.toByte).toArray ++ Array[Byte](13, 10)
      val saved = MultipartSnapshot
        .fromState(
          Saved(17, "árvíz 🦀"),
          Map(
            "index"     -> SnapshotPart(bytes, "Application/Octet-Stream"),
            "state"     -> SnapshotPart(Array.emptyByteArray, "text/plain"),
            "__proto__" -> SnapshotPart(Array[Byte](0, -1, 13), "application/json")
          )
        )
        .toOption
        .get
      val principal             = Principal.Oidc("ユーザー", "issuer", "{}", email = Some("élise@test"))
      val encoded               = MultipartSnapshotCodec.encode(saved, principal).toOption.get
      val (recovered, restored) = MultipartSnapshotCodec.decode(encoded.bytes, encoded.mimeType).toOption.get
      val empty                 =
        MultipartSnapshotCodec.encode(MultipartSnapshot(Json.Null, Map.empty), Principal.Anonymous).toOption.get
      assertTrue(
        recovered == principal,
        restored.decodeState[Saved] == Right(Saved(17, "árvíz 🦀")),
        restored.requirePart("index", "application/octet-stream").exists(_.toVector == bytes.toVector),
        restored.parts.keySet == saved.parts.keySet,
        restored.requirePart("absent", "text/plain").isLeft,
        restored.requirePart("index", "text/plain").isLeft,
        restored.requirePart("index", "text/plain; charset=utf-8").isLeft,
        empty.mimeType.startsWith("multipart/mixed"),
        MultipartSnapshotCodec.decode(empty.bytes, empty.mimeType).exists(_._2.state == Json.Null)
      )
    },
    test("collision checks include body start and appended framing") {
      val bytes   = "--golem-snapshot-0\r\npayload\r\n--golem-snapshot-1--".getBytes("UTF-8")
      val saved   = MultipartSnapshot(Json.Null, Map("index" -> SnapshotPart(bytes, "application/octet-stream")))
      val encoded = MultipartSnapshotCodec.encode(saved, Principal.Anonymous).toOption.get
      assertTrue(
        encoded.mimeType.endsWith("golem-snapshot-2"),
        MultipartSnapshotCodec
          .decode(encoded.bytes, encoded.mimeType)
          .exists(_._2.parts("index").bytes.toVector == bytes.toVector)
      )
    },
    test("rejects malformed metadata, namespaces, MIME and framing without restricting application state") {
      val badMetadata = Vector(
        """[1,{"tag":"anonymous"},null]""",
        envelope.replace("\"version\":1", "\"version\":1.0"),
        envelope.replace("\"version\":1", "\"version\":1e0"),
        envelope.replace("\"version\":1", "\"version\":1,\"version\":1"),
        envelope.replace("\"tag\":\"anonymous\"", "\"tag\":\"anonymous\",\"tag\":\"anonymous\""),
        """{"version":1,"principal":["anonymous"],"state":null}""",
        """{"version":1,"principal":{"tag":"oidc","val":{"sub":"s","issuer":"i","claims":"{}","email":7}},"state":null}""",
        """{"version":1,"principal":{"tag":"agent","val":{"componentId":"10203040-5060-7080-9012-3456789abcde","agentId":"x","agentId":"y"}},"state":null}"""
      )
      val badWire = Vector("part:", "part:a/b", "db:main", "unknown:x", "state").map { name =>
        wire(
          envelope,
          s"--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; name=\"$name\"\r\n\r\nx\r\n"
        )
      } ++ Vector(
        wire(envelope).dropRight(7),
        wire(envelope) ++ Array[Byte](88),
        wire(envelope.replace("null", "\"x\"")).map(b => if (b == 120) 255.toByte else b),
        new String(wire(envelope), "UTF-8")
          .replace("Content-Type: application/json", "Content-Type: application/json\r\nContent-Type: application/json")
          .getBytes("UTF-8"),
        new String(wire(envelope), "UTF-8")
          .replace("Content-Type: application/json", "Content-Type: application/json\r\nUnknown: x")
          .getBytes("UTF-8")
      )
      val ordered =
        """{"state":{"version":1e0,"text":"escaped \"version\":9"},"principal":{"tag":"anonymous"},"\u0076ersion":1}"""
      assertTrue(
        badMetadata.forall(s => MultipartSnapshotCodec.decode(wire(s), "multipart/mixed; boundary=b").isLeft),
        badWire.forall(b => MultipartSnapshotCodec.decode(b, "multipart/mixed; boundary=b").isLeft),
        MultipartSnapshotCodec.decode(wire(ordered), "multipart/mixed; boundary=b").isRight,
        MultipartSnapshotCodec
          .decode(wire(envelope.replace("null", "[17,null]")), "multipart/mixed; boundary=b")
          .isRight,
        MultipartSnapshotCodec
          .encode(
            MultipartSnapshot(Json.Null, Map("a/b" -> SnapshotPart(Array.emptyByteArray, "text/plain"))),
            Principal.Anonymous
          )
          .isLeft,
        MultipartSnapshotCodec
          .encode(
            MultipartSnapshot(
              Json.Null,
              Map("index" -> SnapshotPart(Array.emptyByteArray, "text/plain; charset=utf-8"))
            ),
            Principal.Anonymous
          )
          .isLeft
      )
    },
    test("quoted boundary parameters and state arriving last use the same framing profile") {
      val leading =
        "\r\n--a:b\r\nContent-Disposition: attachment; name=\"part:index\"\r\nContent-Type: text/plain\r\n\r\nx\r\n--a:b\r\nContent-Disposition: attachment; name=\"state\"\r\nContent-Type: application/json\r\n\r\n" + envelope + "\r\n--a:b--"
      val decoded = MultipartSnapshotCodec.decode(leading.getBytes("UTF-8"), "multipart/mixed; boundary=\"a:b\"")
      assertTrue(
        decoded.exists(_._2.parts("index").bytes.toVector == Vector(120.toByte)),
        MultipartSnapshotCodec.decode(wire(envelope), "multipart/mixed; boundary=b; boundary=b").isLeft,
        MultipartSnapshotCodec.decode(leading.getBytes("UTF-8"), "multipart/mixed; boundary=a:b").isLeft,
        MultipartSnapshotCodec.decode(wire(envelope), "multipart/mixed; boundary=\"b").isLeft
      )
    },
    test(
      "generated restore factory skips initialization, rejects wrong modes, and Guest installs only successful restore"
    ) {
      ZIO.fromFuture { implicit ec =>
        val defn = definition
        IndexImpl.initializations = 0
        IndexImpl.restorations = 0
        Guest.resetForTesting()
        val identityInput = SchemaPayload.encode[String]("init")(InputRecordCodec.single[String]("name"))
        js.Dynamic.global
          .selectDynamic("globalThis")
          .updateDynamic("__golemScalaTestHost")(
            js.Dynamic.literal(
              "getSelfMetadata" -> (() =>
                js.Dynamic.literal("agentId" -> js.Dynamic.literal("agentId" -> "multipart-index()"))
              ),
              "parseAgentId" -> ((_: String) =>
                js.Array[js.Any]("multipart-index", js.Dynamic.literal("value" -> identityInput), js.undefined)
              )
            )
          )
        val bytes = (0 to 255).map(_.toByte).toArray ++ Array[Byte](13, 10)
        val saved = MultipartSnapshot
          .fromState(Saved(17, "restore"), Map("index" -> SnapshotPart(bytes, "application/octet-stream")))
          .toOption
          .get
        val context = SnapshotRestoreContext(Vector("init"), "multipart-index()", None, Principal.Anonymous, None)
        for {
          wrong <- defn.snapshotHandlers.get
                     .load(SnapshotPayload(Array.emptyByteArray, "application/octet-stream"), context)
                     .failed
          callsAfterWrong = IndexImpl.restorations
          malformed      <- FutureInterop
                         .fromPromise(
                           Guest.LoadSnapshot.load(
                             jsSnapshot(
                               SnapshotPayload(
                                 wire(envelope.replace("\"version\":1", "\"version\":1e0")),
                                 "multipart/mixed; boundary=b"
                               )
                             )
                           )
                         )
                         .failed
          callsAfterMalformed = IndexImpl.restorations
          missing             = saved.copy(parts = Map.empty)
          failed             <- FutureInterop
                      .fromPromise(
                        Guest.LoadSnapshot
                          .load(jsSnapshot(MultipartSnapshotCodec.encode(missing, Principal.Anonymous).toOption.get))
                      )
                      .failed
          stateAfterFailure = Guest.stateForTesting
          _                <- FutureInterop.fromPromise(
                 Guest.LoadSnapshot.load(
                   jsSnapshot(MultipartSnapshotCodec.encode(saved, Principal.Anonymous).toOption.get)
                 )
               )
          reencoded <- FutureInterop.fromPromise(Guest.SaveSnapshot.save())
          recovered  =
            MultipartSnapshotCodec
              .decode(Array.tabulate(reencoded.payload.length)(i => reencoded.payload(i).toByte), reencoded.mimeType)
              .toOption
              .get
              ._2
          stateAfterSuccess = Guest.stateForTesting
          _                 = Guest.resetForTesting()
        } yield assertTrue(
          wrong.isInstanceOf[SnapshotError],
          callsAfterWrong == 0,
          callsAfterMalformed == 0,
          failed.getMessage.contains("Missing snapshot part"),
          stateAfterFailure == (false, None),
          stateAfterSuccess == (true, Some(Principal.Anonymous)),
          IndexImpl.initializations == 0,
          recovered.requirePart("index", "application/octet-stream").exists(_.toVector == bytes.toVector),
          defn.methodMetadata.size == 1
        )
      }
    }
  ) @@ TestAspect.sequential
}
