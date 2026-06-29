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

package golem.runtime.rpc

import golem.host.SchemaWireInterop
import golem.host.js.schema.{JsSchemaValueTree, JsTypedAgentConfigValue, JsTypedSchemaValue}
import golem.runtime.autowire.SchemaPayload
import golem.schema.{FromSchema, IntoSchema}
import golem.schema.wire.SchemaWire

import scala.scalajs.js.JSConverters._

/**
 * The `golem:agent/host@2.0.0` RPC-boundary codec: encodes the parameter-list
 * value tree and decodes the optional result tree that the v2 `wasm-rpc` host
 * exchanges (`invoke`/`invoke-and-await` take a `schema-value-tree` and return
 * `option<schema-value-tree>`), plus the `typed-schema-value` carrier used for
 * agent-config values and custom errors.
 *
 * This is the RPC value codec for the `golem:core/types@2.0.0` schema model.
 *
 * Mirrors the TS SDK's RPC client (`clientGeneration.ts`):
 *   - method / constructor arguments are encoded as ONE value tree whose root
 *     is the parameter-list record (here: `IntoSchema[In].toValue`, where the
 *     macro shapes `In` as the record of user-supplied parameters);
 *   - `output-schema = unit` => the host returns `none`; `single` =>
 *     `some(tree)` decoded via `FromSchema[Out]`.
 *
 * The optional result of the host `option<schema-value-tree>` is modelled as a
 * Scala [[Option]] here; the raw `@JSImport` host facade (Slice 4d) bridges it
 * to / from `js.UndefOr` at the actual call boundary, keeping this codec free
 * of `js.|` union plumbing.
 */
private[golem] object SchemaRpcCodec {

  // --- arguments (constructor + method input) -------------------------------

  /** Encode the parameter-list value of `In` into a `schema-value-tree`. */
  def encodeArgs[In](input: In)(implicit into: IntoSchema[In]): JsSchemaValueTree =
    SchemaPayload.encode(input)

  /** Decode a parameter-list `schema-value-tree` back into `In`. */
  def decodeArgs[In](tree: JsSchemaValueTree)(implicit from: FromSchema[In]): Either[String, In] =
    SchemaPayload.decode[In](tree).left.map(_.toString)

  // --- results (option<schema-value-tree>) ----------------------------------

  /** `output-schema = unit` => no value tree on the wire. */
  val encodeUnitResult: Option[JsSchemaValueTree] = None

  /** `output-schema = single` => `some(tree)`. */
  def encodeSingleResult[Out](value: Out)(implicit into: IntoSchema[Out]): Option[JsSchemaValueTree] =
    Some(SchemaPayload.encode(value))

  /**
   * Decode a unit result. The host returns `none`; a stray `some` is tolerated
   * and ignored (TS parity), so the only outcome is `()`.
   */
  def decodeUnitResult(result: Option[JsSchemaValueTree]): Either[String, Unit] =
    Right(())

  /**
   * Decode a single-value result; absence is an error for a non-unit method.
   */
  def decodeSingleResult[Out](
    result: Option[JsSchemaValueTree]
  )(implicit from: FromSchema[Out]): Either[String, Out] =
    result match {
      case Some(tree) => SchemaPayload.decode[Out](tree).left.map(_.toString)
      case None       => Left("Expected a return value for a non-unit method output, got none")
    }

  // --- typed-schema-value (agent-config values, custom errors) --------------

  /**
   * Encode `value` into a self-contained `typed-schema-value` (graph + value).
   */
  def encodeTyped[A](value: A)(implicit into: IntoSchema[A]): JsTypedSchemaValue =
    SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(into.toTyped(value)))

  /** Decode a `typed-schema-value` back into `A` using `FromSchema[A]`. */
  def decodeTyped[A](typed: JsTypedSchemaValue)(implicit from: FromSchema[A]): Either[String, A] =
    from
      .fromValue(SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(typed)).value)
      .left
      .map(_.toString)

  /**
   * Build a `typed-agent-config-value` (`path` + `typed-schema-value`) entry.
   */
  def typedConfigValue[A](path: List[String], value: A)(implicit into: IntoSchema[A]): JsTypedAgentConfigValue =
    JsTypedAgentConfigValue(path.toJSArray, encodeTyped(value))
}
