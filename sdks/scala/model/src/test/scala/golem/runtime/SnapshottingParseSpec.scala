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

package golem.runtime

import zio.test._

object SnapshottingParseSpec extends ZIOSpecDefault {
  def spec = suite("SnapshottingParseSpec")(
    test("disabled") {
      assertTrue(Snapshotting.parse("disabled") == Right(Snapshotting.Disabled))
    },
    test("enabled") {
      assertTrue(Snapshotting.parse("enabled") == Right(Snapshotting.Enabled(SnapshottingConfig.Default)))
    },
    test("periodic with seconds") {
      val result = Snapshotting.parse("periodic(5 seconds)")
      assertTrue(result == Right(Snapshotting.Enabled(SnapshottingConfig.Periodic(5000000000L))))
    },
    test("periodic with millis") {
      val result = Snapshotting.parse("periodic(500 millis)")
      assertTrue(result == Right(Snapshotting.Enabled(SnapshottingConfig.Periodic(500000000L))))
    },
    test("periodic with minutes") {
      val result = Snapshotting.parse("periodic(1 minute)")
      assertTrue(result == Right(Snapshotting.Enabled(SnapshottingConfig.Periodic(60000000000L))))
    },
    test("every with valid count") {
      assertTrue(Snapshotting.parse("every(10)") == Right(Snapshotting.Enabled(SnapshottingConfig.EveryN(10))))
    },
    test("every(1)") {
      assertTrue(Snapshotting.parse("every(1)") == Right(Snapshotting.Enabled(SnapshottingConfig.EveryN(1))))
    },
    test("invalid value") {
      assertTrue(Snapshotting.parse("bogus").isLeft)
    },
    test("periodic with invalid duration") {
      assertTrue(Snapshotting.parse("periodic(not-a-duration)").isLeft)
    },
    test("periodic with zero duration") {
      assertTrue(Snapshotting.parse("periodic(0 seconds)").isLeft)
    },
    test("periodic with negative duration") {
      assertTrue(Snapshotting.parse("periodic(-5 seconds)").isLeft)
    },
    test("every with zero") {
      assertTrue(Snapshotting.parse("every(0)").isLeft)
    },
    test("every with negative") {
      assertTrue(Snapshotting.parse("every(-1)").isLeft)
    },
    test("every with non-integer") {
      assertTrue(Snapshotting.parse("every(abc)").isLeft)
    },
    test("case insensitive") {
      assertTrue(Snapshotting.parse("Disabled") == Right(Snapshotting.Disabled))
    },
    test("whitespace trimmed") {
      assertTrue(Snapshotting.parse("  enabled  ") == Right(Snapshotting.Enabled(SnapshottingConfig.Default)))
    }
  )
}
