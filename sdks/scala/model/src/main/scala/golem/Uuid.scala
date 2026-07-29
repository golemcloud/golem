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

package golem

import zio.blocks.schema.Schema

/**
 * Cross-platform UUID representation used by the Scala SDK.
 */
final case class Uuid(highBits: BigInt, lowBits: BigInt)

object Uuid {
  implicit val schema: Schema[Uuid] = Schema.derived

  def random(): Uuid = {
    val high = (randomUnsigned64() & ~BigInt(0xf000)) | BigInt(0x4000)
    val low  = (randomUnsigned64() & ~(BigInt(0xc000) << 48)) | (BigInt(0x8000) << 48)
    Uuid(high, low)
  }

  private def randomUnsigned64(): BigInt = {
    val high = (scala.math.random() * 4294967296.0).toLong
    val low  = (scala.math.random() * 4294967296.0).toLong
    (BigInt(high) << 32) | BigInt(low)
  }

  def toStandardString(uuid: Uuid): String = {
    val hi = uuid.highBits
    val lo = uuid.lowBits
    val p1 = ((hi >> 32) & 0xffffffffL).toLong
    val p2 = ((hi >> 16) & 0xffffL).toLong
    val p3 = (hi & 0xffffL).toLong
    val p4 = ((lo >> 48) & 0xffffL).toLong
    val p5 = lo & 0xffffffffffffL
    f"$p1%08x-$p2%04x-$p3%04x-$p4%04x-$p5%012x"
  }

  def fromStandardString(s: String): Either[String, Uuid] = {
    val parts = s.split('-')
    if (parts.length != 5) Left(s"Invalid UUID string: $s")
    else
      try {
        val p1 = BigInt(parts(0), 16)
        val p2 = BigInt(parts(1), 16)
        val p3 = BigInt(parts(2), 16)
        val p4 = BigInt(parts(3), 16)
        val p5 = BigInt(parts(4), 16)
        val hi = (p1 << 32) | (p2 << 16) | p3
        val lo = (p4 << 48) | p5
        Right(Uuid(hi, lo))
      } catch {
        case _: NumberFormatException => Left(s"Invalid UUID string: $s")
      }
  }
}
