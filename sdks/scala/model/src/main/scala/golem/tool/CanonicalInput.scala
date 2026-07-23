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
  MetadataEnvelope,
  NamedFieldType,
  SchemaConflictError,
  SchemaGraph,
  SchemaType,
  SchemaTypeBody,
  SchemaValue
}
import golem.schema.SchemaBuilder

/**
 * One field of a command's canonical input record: the surface name, its
 * aliases, and the field's collected value graph.
 */
final case class CanonicalInputField(
  name: String,
  aliases: List[String],
  schema: SchemaGraph
)

/**
 * The canonical input model of one command: its ordered fields and the
 * synthesized record schema whose field order is the canonical input order
 * (effective globals → positionals → tail → options → flags).
 */
final case class CanonicalInputModel(
  fields: List[CanonicalInputField],
  recordSchema: SchemaGraph
) {

  /**
   * Decode a positional input record against the canonical fields, pairing each
   * field with its value by index.
   */
  def decodeRecord(
    value: SchemaValue
  ): Either[CanonicalInputDecodeError, List[CanonicalInputValue]] =
    value match {
      case SchemaValue.RecordValue(values) =>
        if (values.length != fields.length)
          Left(CanonicalInputDecodeError.FieldCountMismatch(fields.length, values.length))
        else
          Right(fields.zip(values).map { case (field, v) =>
            CanonicalInputValue(field.name, field.aliases, field.schema, v)
          })
      case _ => Left(CanonicalInputDecodeError.ExpectedRecord)
    }
}

object CanonicalInputModel {

  def fromFields(fields: List[CanonicalInputField]): Either[ToolBuildError, CanonicalInputModel] =
    recordSchema(fields).map(CanonicalInputModel(fields, _))

  /**
   * Synthesize the canonical input record schema for the given fields: field
   * graphs are checked for closedness, their definitions merged, and the roots
   * assembled into a record in field order.
   */
  private[tool] def recordSchema(
    fields: List[CanonicalInputField]
  ): Either[ToolBuildError, SchemaGraph] = {
    val it = fields.iterator
    while (it.hasNext) {
      val field = it.next()
      ToolGraphs.checkGraphClosed(field.schema, s"canonical input field ${"\"" + field.name + "\""}") match {
        case Left(err) => return Left(err)
        case Right(_)  => ()
      }
    }
    val mergedDefs =
      try Right(SchemaBuilder.mergeGraphDefs(fields.map(_.schema)))
      catch {
        case e: SchemaConflictError => Left(ToolBuildError.EncodeError(e.getMessage))
      }
    mergedDefs.flatMap { defs =>
      val graph = SchemaGraph(
        defs,
        SchemaType(
          SchemaTypeBody.RecordType(
            fields.map(field => NamedFieldType(field.name, field.schema.root, MetadataEnvelope.empty))
          )
        )
      )
      ToolGraphs.checkGraphClosed(graph, "canonical input record").map(_ => graph)
    }
  }
}

/** A canonical input field paired with its decoded value. */
final case class CanonicalInputValue(
  name: String,
  aliases: List[String],
  schema: SchemaGraph,
  value: SchemaValue
)

sealed trait CanonicalInputDecodeError extends Product with Serializable {
  def message: String
}

object CanonicalInputDecodeError {
  final case class Model(error: ToolBuildError) extends CanonicalInputDecodeError {
    def message: String = error.message
  }
  case object ExpectedRecord extends CanonicalInputDecodeError {
    def message: String = "tool input must be a positional record"
  }
  final case class FieldCountMismatch(expected: Int, actual: Int) extends CanonicalInputDecodeError {
    def message: String =
      s"tool input record has $actual fields, expected $expected canonical fields"
  }
}
