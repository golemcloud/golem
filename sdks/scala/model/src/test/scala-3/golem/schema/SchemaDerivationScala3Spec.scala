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

package golem.schema

import zio.blocks.schema.Schema
import zio.test.Assertion._
import zio.test._

/** Derivation cases for stdlib `Either` and tuples. */
object SchemaDerivationScala3Spec extends ZIOSpecDefault {
  import SchemaTypeBody._

  implicit val eitherSchema: Schema[Either[String, Int]] = Schema.derived
  implicit val tuple2Schema: Schema[(Int, String)]       = Schema.derived

  private def rootBody[A](implicit s: IntoSchema[A]): SchemaTypeBody = s.graph.root.body

  private def roundTrip[A](value: A)(implicit into: IntoSchema[A], from: FromSchema[A]): Either[FromSchemaError, A] =
    from.fromValue(into.toValue(value))

  override def spec: Spec[TestEnvironment, Any] =
    suite("SchemaDerivationScala3Spec")(
      test("Either derives result and round-trips Left/Right") {
        assertTrue(rootBody[Either[String, Int]] == ResultType(Some(t.s32), Some(t.string))) &&
        assert(roundTrip[Either[String, Int]](Right(5)))(isRight(equalTo(Right(5): Either[String, Int]))) &&
        assert(roundTrip[Either[String, Int]](Left("e")))(isRight(equalTo(Left("e"): Either[String, Int])))
      },
      test("tuple derives tuple-type and round-trips") {
        assertTrue(rootBody[(Int, String)] == TupleType(List(t.s32, t.string))) &&
        assert(roundTrip((7, "x")))(isRight(equalTo((7, "x"))))
      }
    )
}
