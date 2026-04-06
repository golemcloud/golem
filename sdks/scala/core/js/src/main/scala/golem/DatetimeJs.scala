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

import golem.host.js.JsDatetime

/**
 * Scala.js constructors for [[Datetime]].
 *
 * This is the only place where the JS representation is constructed; public
 * scheduling APIs accept [[Datetime]] instead of `js.*`.
 */
object DatetimeJs {

  /**
   * Construct a Datetime from a JS value understood by the host
   * (router/runtime).
   *
   * This is intentionally "unsafe": it trusts the shape expected by the host.
   */
  def unsafeFromHost(datetime: JsDatetime): Datetime = {
    val secs  = datetime.seconds.toString.toDouble
    val nanos = datetime.nanoseconds.toDouble
    Datetime.fromEpochMillis(secs * 1000.0 + nanos / 1000000.0)
  }

  /**
   * Convenience constructor matching existing tests (`{ ts: <number> }`).
   *
   * Note: the exact shape is host-defined; this helper exists to avoid leaking
   * `js.Dynamic` into user code.
   */
  def fromTs(ts: Double): Datetime =
    Datetime.fromEpochMillis(ts)
}
