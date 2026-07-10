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

package golem.tool

import golem.schema.{
  FromSchema,
  IntoSchema,
  SchemaEncodeError,
  SchemaGraph,
  SchemaType,
  SchemaTypeBody,
  SchemaValue,
  TypedSchemaValue
}

import scala.collection.immutable.ListMap

/**
 * Runtime helpers referenced by the macro-derived [[ToolErrorSchema]]
 * instances: payload encode/decode plus the canonical unit payload used by
 * error cases without a payload field.
 */
object ToolErrorSupport {

  /** The payload carrier of a no-payload error case: the empty tuple. */
  val unitPayloadGraph: SchemaGraph =
    SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.TupleType(Nil)))

  val unitPayload: TypedSchemaValue =
    TypedSchemaValue(unitPayloadGraph, SchemaValue.TupleValue(Nil))

  def encodeUnitPayload: Either[String, TypedSchemaValue] = Right(unitPayload)

  def encodePayload[A](value: A, into: IntoSchema[A]): Either[String, TypedSchemaValue] =
    try Right(into.toTyped(value))
    catch {
      case e: SchemaEncodeError => Left(e.message)
    }

  def decodePayload[A](value: TypedSchemaValue, from: FromSchema[A]): Either[String, A] =
    from.fromValue(value.value).left.map(_.message)

  /** Whether a payload value decodes as the unit (no-payload) carrier. */
  def isUnitPayload(value: TypedSchemaValue): Boolean =
    value.value == SchemaValue.TupleValue(Nil)

  val unmatchedPayload: String =
    "remote tool error payload did not match any declared error case"
}
