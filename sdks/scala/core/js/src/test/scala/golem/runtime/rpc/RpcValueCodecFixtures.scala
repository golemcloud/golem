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

package golem.runtime.rpc

import zio.blocks.schema.Schema

final case class Point(x: Int, y: Int)
object Point { implicit val schema: Schema[Point] = Schema.derived }

final case class Labels(values: Map[String, Int])
object Labels { implicit val schema: Schema[Labels] = Schema.derived }

sealed trait Color
object Color {
  case object Red  extends Color
  case object Blue extends Color
  implicit val schema: Schema[Color] = Schema.derived
}
