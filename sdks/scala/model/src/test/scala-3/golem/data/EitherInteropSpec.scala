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

object EitherInteropSpec extends ZIOSpecDefault {
  implicit val eitherStringIntSchema: Schema[Either[String, Int]] = Schema.derived

  override def spec: Spec[TestEnvironment, Any] =
    suite("EitherInteropSpec")(
      test("maps Either to ResultType") {
        val dt = DataInterop.schemaToDataType(eitherStringIntSchema)
        assertTrue(dt == DataType.ResultType(ok = Some(DataType.IntType), err = Some(DataType.StringType)))
      },
      test("round trips Either Right values") {
        val right: Either[String, Int] = Right(42)
        val encoded                    = DataInterop.toData(right)
        assertTrue(encoded == DataValue.ResultValue(Right(DataValue.IntValue(42)))) &&
        assert(DataInterop.fromData[Either[String, Int]](encoded))(isRight(equalTo(Right(42))))
      },
      test("round trips Either Left values") {
        val left: Either[String, Int] = Left("error")
        val encoded                   = DataInterop.toData(left)
        assertTrue(encoded == DataValue.ResultValue(Left(DataValue.StringValue("error")))) &&
        assert(DataInterop.fromData[Either[String, Int]](encoded))(isRight(equalTo(Left("error"))))
      }
    )
}
