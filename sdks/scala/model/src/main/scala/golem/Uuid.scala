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

/**
 * Cross-platform UUID representation used by the Scala SDK.
 */
final case class Uuid(highBits: BigInt, lowBits: BigInt)

object Uuid {
  implicit val schema: Schema[Uuid] = Schema.derived

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
