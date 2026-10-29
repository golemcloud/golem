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

package golem

import java.util.UUID

/**
 * The canonical text form of a UUID: lowercase hexadecimal digits in the
 * hyphenated 8-4-4-4-12 layout. `UUID.toString` produces this form.
 * `UUID.fromString` also accepts other forms, so decoders that must reject
 * non-canonical text use [[CanonicalUuid.parse]].
 */
private[golem] object CanonicalUuid {
  private val Pattern = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}".r

  def parse(text: String): Either[String, UUID] =
    if (Pattern.matches(text)) Right(UUID.fromString(text))
    else Left(s"Invalid UUID string, expected lowercase hyphenated form: $text")
}
