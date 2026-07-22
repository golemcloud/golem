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

import scala.concurrent.duration.{Duration, FiniteDuration}

/** Cache policy for read-only agent methods. */
sealed trait CachePolicy
object CachePolicy {
  case object NoCache               extends CachePolicy
  case object UntilWrite            extends CachePolicy
  final case class Ttl(nanos: Long) extends CachePolicy

  /**
   * Parse a cache policy from a string used in `@readOnly(cache = ...)`
   * annotation arguments.
   *
   * Accepted forms:
   *   - "no-cache"
   *   - "until-write"
   *   - "ttl(<duration>)" — for example "ttl(30 seconds)"
   */
  def parse(value: String): Either[String, CachePolicy] = {
    val trimmed = value.trim.toLowerCase
    trimmed match {
      case "no-cache"                                   => Right(NoCache)
      case "until-write"                                => Right(UntilWrite)
      case s if s.startsWith("ttl(") && s.endsWith(")") =>
        val inner = s.substring("ttl(".length, s.length - 1).trim
        try {
          val dur = Duration(inner)
          if (!dur.isFinite) Left(s"ttl duration must be finite, got: $inner")
          else {
            val fd = FiniteDuration(dur.toNanos, scala.concurrent.duration.NANOSECONDS)
            if (fd.toNanos <= 0) Left(s"ttl duration must be positive, got: $inner")
            else Right(Ttl(fd.toNanos))
          }
        } catch {
          case _: NumberFormatException =>
            Left(s"invalid duration in ttl('$inner'). Use formats like '5 seconds', '500 millis', '1 minute'")
        }
      case other =>
        Left(
          s"invalid cache policy '$other'. Valid values: no-cache, until-write, ttl(<duration>)"
        )
    }
  }
}

/**
 * Configuration attached to a read-only agent method.
 *
 * @param cachePolicy
 *   The cache policy the host may apply to invocations of this method.
 * @param usesPrincipal
 *   Whether the method receives the invocation principal (derived from the
 *   method signature; the user does not set this directly).
 */
final case class ReadOnlyConfig(
  cachePolicy: CachePolicy,
  usesPrincipal: Boolean
)
