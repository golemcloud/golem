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

import golem.runtime.wit.WitResult

/**
 * This is a thin alias/wrapper over [[golem.runtime.wit.WitResult]] which
 * contains the full implementation.
 */
object Result {
  type Result[+Ok, +Err] = WitResult[Ok, Err]

  def ok[Ok](value: Ok): Result[Ok, Nothing] =
    WitResult.ok(value)

  def err[Err](value: Err): Result[Nothing, Err] =
    WitResult.err(value)

  def fromEither[Err, Ok](either: Either[Err, Ok]): Result[Ok, Err] =
    WitResult.fromEither(either)

  def fromOption[Ok](value: Option[Ok], orElse: => String): Result[Ok, String] =
    WitResult.fromOption(value, orElse)
}
