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

package golem.data

import zio.blocks.schema.Schema

final case class UByte(value: Short) extends AnyVal
object UByte {
  implicit val schema: Schema[UByte] = Schema.derived
}

final case class UShort(value: Int) extends AnyVal
object UShort {
  implicit val schema: Schema[UShort] = Schema.derived
}

final case class UInt(value: Long) extends AnyVal
object UInt {
  implicit val schema: Schema[UInt] = Schema.derived
}

final case class ULong(value: BigInt)
object ULong {
  implicit val schema: Schema[ULong] = Schema.derived
}
