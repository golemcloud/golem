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
import golem.runtime.annotations.agentImplementation
import zio.blocks.schema.json.JsonCodecDeriver
import zio.blocks.schema.Schema

import scala.annotation.unused
import scala.concurrent.Future
@agentImplementation()
final class JsonPromiseDemoImpl(@unused private val name: String) extends JsonPromiseDemo {

  private var savedPromiseId: Option[HostApi.PromiseId] = None

  override def jsonRoundtrip(): Future[String] = Future.successful {
    val sb    = new StringBuilder
    val codec = implicitly[Schema[PromisePayload]].derive(JsonCodecDeriver)
    sb.append("=== JSON Promise Roundtrip Demo ===\n")

    val payload = PromisePayload("hello from json promise", 42)
    val encoded = codec.encode(payload)
    sb.append(s"encoded JSON bytes length=${encoded.length}\n")

    codec.decode(encoded) match {
      case Right(decoded) =>
        sb.append(s"decoded: message=${decoded.message}, count=${decoded.count}\n")
        sb.append(s"roundtrip matches=${decoded == payload}\n")
      case Left(err) =>
        sb.append(s"decode error: $err\n")
    }

    val promiseId = HostApi.createPromise()
    sb.append(s"createPromise ok\n")

    val completed = HostApi.completePromiseJson(promiseId, payload)
    sb.append(s"completePromiseJson ok=$completed\n")

    val rawPromise = HostApi.createPromise()
    val rawData    = "raw-bytes-test".getBytes("UTF-8")
    val rawOk      = HostApi.completePromise(rawPromise, rawData)
    sb.append(s"completePromise (raw bytes) ok=$rawOk\n")

    savedPromiseId = Some(HostApi.createPromise())
    sb.append(s"created saved promise for blockingDemo\n")

    sb.result()
  }

  override def blockingDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Promise Complete + Idempotency Demo ===\n")

    savedPromiseId match {
      case Some(promiseId) =>
        val data = "blocking-test-data".getBytes("UTF-8")
        val ok   = HostApi.completePromise(promiseId, data)
        sb.append(s"completePromise (cross-invocation) ok=$ok\n")

        val ok2 = HostApi.completePromise(promiseId, "duplicate".getBytes("UTF-8"))
        sb.append(s"completePromise (duplicate) ok=$ok2 (expected false)\n")
        savedPromiseId = None

      case None =>
        sb.append("No saved promise. Call jsonRoundtrip() first.\n")
    }

    val key = HostApi.generateIdempotencyKey()
    sb.append(s"generateIdempotencyKey=$key\n")

    sb.result()
  }
}
