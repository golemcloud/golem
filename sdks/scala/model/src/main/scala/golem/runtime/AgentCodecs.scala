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

import golem.schema.{
  FromSchema,
  FromSchemaError,
  IntoSchema,
  MetadataEnvelope,
  NamedFieldType,
  SchemaBuilder,
  SchemaGraph,
  SchemaType,
  SchemaTypeBody,
  SchemaTypeDef,
  SchemaValue
}

import scala.collection.immutable.ListMap

/**
 * A single user-supplied parameter's value codec + metadata. The macro summons
 * `IntoSchema`/`FromSchema` for the parameter type and erases to `Any`.
 */
final case class ParamCodec(
  name: String,
  into: IntoSchema[Any],
  from: FromSchema[Any],
  metadata: MetadataEnvelope = MetadataEnvelope.empty
) {
  def parameterMetadata: ParameterMetadata =
    ParameterMetadata(name, FieldSource.UserSupplied, into.graph, metadata)
}

/**
 * The constructor/method input codec for the `golem:agent@2.0.0` boundary.
 *
 * The wire value is ALWAYS a [[SchemaValue.RecordValue]] whose fields are the
 * user-supplied parameters in declaration order — including the zero-arg
 * (`RecordValue(Nil)`) and single-arg (`RecordValue(List(field))`) cases. This
 * mirrors the TS SDK's `encodeInputRecord` / `decodeInputRecord`.
 *
 * Extends [[IntoSchema]] + [[FromSchema]] so it plugs directly into
 * `SchemaPayload` / `SchemaRpcCodec` at the host boundary.
 */
trait InputRecordCodec[A] extends IntoSchema[A] with FromSchema[A] {

  /**
   * The user-supplied parameters (each consumes one record field, in order).
   */
  def userParams: List[ParameterMetadata]

  /** The schema-native input description (the v2 `input-schema`). */
  final def inputMetadata: InputMetadata = InputMetadata(userParams)
}

object InputRecordCodec {

  /**
   * Build the combined record [[SchemaGraph]] from the per-parameter graphs.
   * (Used to satisfy the [[IntoSchema]] contract; the value path itself does
   * not need it.)
   */
  private def recordGraph(params: List[ParameterMetadata]): SchemaGraph = {
    val defs: ListMap[String, SchemaTypeDef] = SchemaBuilder.mergeGraphDefs(params.map(_.graph))
    val fields                               = params.map(p => NamedFieldType(p.name, p.graph.root, p.metadata))
    SchemaGraph(defs, SchemaType(SchemaTypeBody.RecordType(fields)))
  }

  /** No user-supplied parameters: the input is the empty record. */
  val unit: InputRecordCodec[Unit] = new InputRecordCodec[Unit] {
    override val userParams: List[ParameterMetadata]                          = Nil
    override lazy val graph: SchemaGraph                                      = recordGraph(Nil)
    override def toValue(value: Unit): SchemaValue                            = SchemaValue.RecordValue(Nil)
    override def fromValue(value: SchemaValue): Either[FromSchemaError, Unit] =
      value match {
        case SchemaValue.RecordValue(Nil)    => Right(())
        case SchemaValue.RecordValue(fields) =>
          Left(FromSchemaError(s"Expected a 0-field record for agent input, got ${fields.length}"))
        case other => Left(FromSchemaError(s"Expected record value for agent input, got $other"))
      }
  }

  /** Exactly one user-supplied parameter: a one-field record. */
  def single[A](name: String, metadata: MetadataEnvelope = MetadataEnvelope.empty)(implicit
    into: IntoSchema[A],
    from: FromSchema[A]
  ): InputRecordCodec[A] = new InputRecordCodec[A] {
    override val userParams: List[ParameterMetadata] =
      List(ParameterMetadata(name, FieldSource.UserSupplied, into.graph, metadata))
    override lazy val graph: SchemaGraph                                   = recordGraph(userParams)
    override def toValue(value: A): SchemaValue                            = SchemaValue.RecordValue(List(into.toValue(value)))
    override def fromValue(value: SchemaValue): Either[FromSchemaError, A] =
      value match {
        case SchemaValue.RecordValue(field :: Nil) => from.fromValue(field)
        case SchemaValue.RecordValue(fields)       =>
          Left(FromSchemaError(s"Expected a 1-field record for agent input, got ${fields.length}"))
        case other =>
          Left(FromSchemaError(s"Expected record value for agent input, got $other"))
      }
  }

  /**
   * Multiple user-supplied parameters: a record encoded/decoded by position.
   */
  def fromParams(params: List[ParamCodec]): InputRecordCodec[Vector[Any]] = new InputRecordCodec[Vector[Any]] {
    private val paramsArr                            = params.toVector
    override val userParams: List[ParameterMetadata] = params.map(_.parameterMetadata)
    override lazy val graph: SchemaGraph             = recordGraph(userParams)

    override def toValue(value: Vector[Any]): SchemaValue = {
      if (value.length != paramsArr.length)
        throw new IllegalArgumentException(
          s"Expected ${paramsArr.length} arguments for agent input, got ${value.length}"
        )
      val fields = List.newBuilder[SchemaValue]
      var idx    = 0
      while (idx < paramsArr.length) {
        fields += paramsArr(idx).into.toValue(value(idx))
        idx += 1
      }
      SchemaValue.RecordValue(fields.result())
    }

    override def fromValue(value: SchemaValue): Either[FromSchemaError, Vector[Any]] =
      value match {
        case SchemaValue.RecordValue(fields) =>
          if (fields.length != paramsArr.length)
            Left(
              FromSchemaError(s"Expected a ${paramsArr.length}-field record for agent input, got ${fields.length}")
            )
          else {
            val builder                      = Vector.newBuilder[Any]
            val fieldsVec                    = fields.toVector
            var idx                          = 0
            var err: Option[FromSchemaError] = None
            while (idx < paramsArr.length && err.isEmpty) {
              paramsArr(idx).from.fromValue(fieldsVec(idx)) match {
                case Right(v) => builder += v
                case Left(e)  => err = Some(e)
              }
              idx += 1
            }
            err.toLeft(builder.result())
          }
        case other => Left(FromSchemaError(s"Expected record value for agent input, got $other"))
      }
  }
}

/**
 * A method's output codec for the `golem:agent@2.0.0` boundary. `Unit` encodes
 * the absence of a value (the host returns `none`); `Single` carries the value
 * codec (`some(tree)`).
 */
final case class OutputCodec[A] private (
  into: Option[IntoSchema[A]],
  from: Option[FromSchema[A]],
  metadata: OutputMetadata
)

object OutputCodec {
  def unit[A]: OutputCodec[A] = OutputCodec[A](None, None, OutputMetadata.Unit)

  def single[A](implicit into: IntoSchema[A], from: FromSchema[A]): OutputCodec[A] =
    OutputCodec[A](Some(into), Some(from), OutputMetadata.Single(into.graph))
}
