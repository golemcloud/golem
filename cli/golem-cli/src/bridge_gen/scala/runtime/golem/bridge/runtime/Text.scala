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

/**
 * A bare `text` schema value: the text with an optional language code. The
 * restrictions of the schema (languages, length, regex) are checked by the
 * server.
 *
 * Mirrors the Golem Scala SDK's `golem.schema.Text`.
 */
final case class Text(text: String, language: Option[String] = None)

object Text {
  def toSchemaValue(value: Text): SchemaValue =
    SchemaValue.TextValue(value.text, value.language)

  def fromSchemaValue(value: SchemaValue): Text = value match {
    case SchemaValue.TextValue(text, language) => Text(text, language)
    case other                                 => throw BridgeException(s"Expected text value, got $other")
  }
}
