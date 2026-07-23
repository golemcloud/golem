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

package golem.bridge.runtime

import java.time.Instant

/**
 * Scheduled invocation time used by the generated client's `scheduleAt`
 * methods. This is a distinct concept from a schema `datetime` field value
 * (which maps to `java.time.Instant`): this type only describes *when* a
 * scheduled invocation should run.
 *
 * Mirrors the Golem Scala SDK's `golem.Datetime` API (epoch-millis based).
 */
final case class Datetime(epochMillis: Long) {

  /** Convert to a `java.time.Instant`. */
  def toInstant: Instant = Instant.ofEpochMilli(epochMillis)

  /** ISO-8601 / RFC-3339 representation, as expected by the REST API. */
  def toIsoString: String = toInstant.toString
}

object Datetime {

  /** Current time. */
  def now: Datetime = fromEpochMillis(System.currentTimeMillis())

  /** Epoch-millis constructor (recommended). */
  def fromEpochMillis(ms: Long): Datetime = Datetime(ms)

  /** Epoch-seconds constructor (convenience). */
  def fromEpochSeconds(seconds: Long): Datetime = fromEpochMillis(seconds * 1000L)

  def fromInstant(instant: Instant): Datetime = fromEpochMillis(instant.toEpochMilli)

  /** A time relative to now (milliseconds). */
  def afterMillis(deltaMs: Long): Datetime =
    fromEpochMillis(System.currentTimeMillis() + deltaMs)

  /** A time relative to now (seconds). */
  def afterSeconds(deltaSeconds: Long): Datetime = afterMillis(deltaSeconds * 1000L)
}
