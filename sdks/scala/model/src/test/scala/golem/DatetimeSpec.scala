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

package golem

import zio.test._

object DatetimeSpec extends ZIOSpecDefault {
  override def spec: Spec[TestEnvironment, Any] =
    suite("DatetimeSpec")(
      test("fromEpochMillis preserves millis") {
        val dt = Datetime.fromEpochMillis(1234.5)
        assertTrue(dt.epochMillis == 1234.5)
      },
      test("fromEpochSeconds scales to millis") {
        val dt = Datetime.fromEpochSeconds(1.5)
        assertTrue(dt.epochMillis == 1500.0)
      },
      test("afterMillis returns time after now") {
        val start = System.currentTimeMillis().toDouble
        val dt    = Datetime.afterMillis(250.0)
        assertTrue(dt.epochMillis >= start + 200.0)
      },
      test("afterSeconds delegates to afterMillis") {
        val start = System.currentTimeMillis().toDouble
        val dt    = Datetime.afterSeconds(0.5)
        assertTrue(dt.epochMillis >= start + 400.0)
      },
      test("now returns current-ish time") {
        val start = System.currentTimeMillis().toDouble
        val dt    = Datetime.now
        assertTrue(dt.epochMillis >= start - 5.0)
      }
    )
}
