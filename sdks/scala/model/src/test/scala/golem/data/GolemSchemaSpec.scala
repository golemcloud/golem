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
import zio.test._
import zio.test.Assertion._

object GolemSchemaSpec extends ZIOSpecDefault {
  final case class Person(name: String, age: Int)
  implicit val personSchema: Schema[Person] = Schema.derived

  override def spec: Spec[TestEnvironment, Any] =
    suite("GolemSchemaSpec")(
      test("unit schema encodes and decodes empty tuples") {
        val schema  = GolemSchema.unitGolemSchema
        val encoded = schema.encode(())
        val decoded = encoded.flatMap(schema.decode)

        assert(encoded)(isRight(equalTo(StructuredValue.Tuple(Nil)))) &&
        assert(decoded)(isRight(equalTo(())))
      },
      test("unit schema rejects non-empty tuples") {
        val schema = GolemSchema.unitGolemSchema
        val value  =
          StructuredValue.Tuple(List(NamedElementValue("value", ElementValue.Component(DataValue.IntValue(1)))))

        assert(schema.decode(value))(isLeft)
      },
      test("tuple2 schema round-trips") {
        val schema  = implicitly[GolemSchema[(Int, String)]]
        val value   = (42, "zio")
        val encoded = schema.encode(value)
        val decoded = encoded.flatMap(schema.decode)

        assert(decoded)(isRight(equalTo(value)))
      },
      test("tuple2 schema reports missing field and wrong element type") {
        val schema  = implicitly[GolemSchema[(Int, String)]]
        val missing = StructuredValue.Tuple(
          List(NamedElementValue("arg0", ElementValue.Component(DataValue.IntValue(1))))
        )
        val wrong = StructuredValue.Tuple(
          List(
            NamedElementValue("arg0", ElementValue.Component(DataValue.IntValue(1))),
            NamedElementValue("arg1", ElementValue.UnstructuredText(UnstructuredTextValue.Inline("oops", None)))
          )
        )

        assert(schema.decode(missing))(isLeft) &&
        assert(schema.decode(wrong))(isLeft)
      },
      test("tuple3 schema round-trips") {
        val schema  = implicitly[GolemSchema[(Int, String, Boolean)]]
        val value   = (7, "ok", true)
        val encoded = schema.encode(value)
        val decoded = encoded.flatMap(schema.decode)

        assert(decoded)(isRight(equalTo(value)))
      },
      test("tuple schemas reject multimodal values") {
        val tuple2     = implicitly[GolemSchema[(Int, String)]]
        val tuple3     = implicitly[GolemSchema[(Int, String, Boolean)]]
        val multimodal = StructuredValue.Multimodal(
          List(NamedElementValue("value", ElementValue.Component(DataValue.StringValue("oops"))))
        )

        assert(tuple2.decode(multimodal))(isLeft) &&
        assert(tuple3.decode(multimodal))(isLeft)
      },
      test("GolemSchema.apply summons implicits") {
        val schema = GolemSchema[Person]
        assertTrue(schema.schema.isInstanceOf[StructuredSchema])
      },
      test("derived schemas round-trip case classes") {
        val schema  = implicitly[GolemSchema[Person]]
        val value   = Person("Ada", 37)
        val encoded = schema.encode(value)
        val decoded = encoded.flatMap(schema.decode)

        assert(decoded)(isRight(equalTo(value)))
      },
      test("derived schemas reject invalid structured values") {
        val schema = implicitly[GolemSchema[Person]]
        val empty  = StructuredValue.Tuple(Nil)
        val wrong  = StructuredValue.Tuple(
          List(NamedElementValue("value", ElementValue.UnstructuredText(UnstructuredTextValue.Inline("oops", None))))
        )
        val multimodal = StructuredValue.Multimodal(
          List(NamedElementValue("value", ElementValue.Component(DataValue.StringValue("oops"))))
        )

        assert(schema.decode(empty))(isLeft) &&
        assert(schema.decode(wrong))(isLeft) &&
        assert(schema.decode(multimodal))(isLeft)
      }
    )
}
