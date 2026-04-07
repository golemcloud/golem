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

import scala.concurrent.duration.{Duration, FiniteDuration}

sealed trait Snapshotting
object Snapshotting {
  case object Disabled                                 extends Snapshotting
  final case class Enabled(config: SnapshottingConfig) extends Snapshotting

  // String constants for use in @agentDefinition(snapshotting = ...) annotations.
  // These provide IDE completion and avoid typos in stringly-typed annotation parameters.
  final val disabled: String             = "disabled"
  final val enabled: String              = "enabled"
  def periodic(duration: String): String = s"periodic($duration)"
  def everyN(count: Int): String         = s"every($count)"

  def parse(value: String): Either[String, Snapshotting] = {
    val trimmed = value.trim.toLowerCase
    trimmed match {
      case "disabled"                                        => Right(Disabled)
      case "enabled"                                         => Right(Enabled(SnapshottingConfig.Default))
      case s if s.startsWith("periodic(") && s.endsWith(")") =>
        val inner = s.substring("periodic(".length, s.length - 1).trim
        try {
          val dur = Duration(inner)
          if (!dur.isFinite) Left(s"periodic duration must be finite, got: $inner")
          else {
            val fd = FiniteDuration(dur.toNanos, scala.concurrent.duration.NANOSECONDS)
            if (fd.toNanos <= 0) Left(s"periodic duration must be positive, got: $inner")
            else Right(Enabled(SnapshottingConfig.Periodic(fd.toNanos)))
          }
        } catch {
          case _: NumberFormatException =>
            Left(s"invalid duration in periodic('$inner'). Use formats like '5 seconds', '500 millis', '1 minute'")
        }
      case s if s.startsWith("every(") && s.endsWith(")") =>
        val inner = s.substring("every(".length, s.length - 1).trim
        try {
          val n = inner.toInt
          if (n <= 0) Left(s"every count must be positive, got: $n")
          else Right(Enabled(SnapshottingConfig.EveryN(n)))
        } catch {
          case _: NumberFormatException => Left(s"invalid count in every('$inner'), expected a positive integer")
        }
      case other =>
        Left(
          s"invalid snapshotting value '$other'. Valid values: disabled, enabled, periodic(<duration>), every(<count>)"
        )
    }
  }
}

sealed trait SnapshottingConfig
object SnapshottingConfig {
  case object Default                    extends SnapshottingConfig
  final case class Periodic(nanos: Long) extends SnapshottingConfig
  final case class EveryN(count: Int)    extends SnapshottingConfig
}
