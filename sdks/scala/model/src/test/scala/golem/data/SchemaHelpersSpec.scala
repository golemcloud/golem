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

import zio.test._
import zio.test.Assertion._

object SchemaHelpersSpec extends ZIOSpecDefault {
  override def spec: Spec[TestEnvironment, Any] =
    suite("SchemaHelpersSpec")(
      test("extracts single element schema") {
        val schema = StructuredSchema.Tuple(
          List(NamedElementSchema("value", ElementSchema.Component(DataType.StringType)))
        )
        assert(SchemaHelpers.singleElementSchema(schema))(
          isRight(equalTo(ElementSchema.Component(DataType.StringType)))
        )
      },
      test("rejects empty or multi-element schema") {
        val empty = StructuredSchema.Tuple(Nil)
        val multi =
          StructuredSchema.Tuple(
            List(
              NamedElementSchema("a", ElementSchema.Component(DataType.IntType)),
              NamedElementSchema("b", ElementSchema.Component(DataType.BoolType))
            )
          )
        val multimodal = StructuredSchema.Multimodal(
          List(NamedElementSchema("value", ElementSchema.Component(DataType.StringType)))
        )

        assert(SchemaHelpers.singleElementSchema(empty))(isLeft) &&
        assert(SchemaHelpers.singleElementSchema(multi))(isLeft) &&
        assert(SchemaHelpers.singleElementSchema(multimodal))(isLeft)
      },
      test("extracts single element value") {
        val value = StructuredValue.Tuple(
          List(NamedElementValue("value", ElementValue.Component(DataValue.StringValue("ok"))))
        )
        assert(SchemaHelpers.singleElementValue(value))(
          isRight(equalTo(ElementValue.Component(DataValue.StringValue("ok"))))
        )
      },
      test("rejects empty or multi-element values") {
        val empty = StructuredValue.Tuple(Nil)
        val multi = StructuredValue.Tuple(
          List(
            NamedElementValue("a", ElementValue.Component(DataValue.IntValue(1))),
            NamedElementValue("b", ElementValue.Component(DataValue.IntValue(2)))
          )
        )
        val multimodal = StructuredValue.Multimodal(
          List(NamedElementValue("value", ElementValue.Component(DataValue.StringValue("ok"))))
        )

        assert(SchemaHelpers.singleElementValue(empty))(isLeft) &&
        assert(SchemaHelpers.singleElementValue(multi))(isLeft) &&
        assert(SchemaHelpers.singleElementValue(multimodal))(isLeft)
      },
      test("wraps element value into structured value") {
        val element = ElementValue.Component(DataValue.BoolValue(true))
        assert(SchemaHelpers.wrapElementValue(element))(equalTo(StructuredValue.single(element)))
      }
    )
}
