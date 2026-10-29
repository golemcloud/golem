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

package golem.schema

/**
 * The schema `char` kind is a Unicode scalar value, and `string` and every
 * other textual leaf (text and its language, path, url, MIME type, quantity
 * unit, union tag) is valid Unicode text. A Scala `Char` or `String` can hold a
 * surrogate code unit that is not part of a pair, which none of these kinds can
 * represent.
 */
private[golem] object UnicodeText {
  def isScalarValue(codePoint: Int): Boolean =
    codePoint >= 0 && codePoint <= 0x10ffff && (codePoint < 0xd800 || codePoint > 0xdfff)

  /** The index of the first unpaired surrogate in `value`, or -1. */
  def loneSurrogateIndex(value: String): Int = {
    var index = 0
    while (index < value.length) {
      val current = value.charAt(index)
      if (Character.isHighSurrogate(current)) {
        if (index + 1 < value.length && Character.isLowSurrogate(value.charAt(index + 1))) index += 2
        else return index
      } else if (Character.isLowSurrogate(current)) return index
      else index += 1
    }
    -1
  }

  def encodeChar(value: Char): Int = {
    if (Character.isSurrogate(value))
      throw SchemaEncodeError(f"char U+${value.toInt}%04X is a surrogate, not a Unicode scalar value")
    value.toInt
  }

  def encodeString(value: String): String = encodeText("string", value)

  /**
   * Checks a textual leaf; `what` names it in the error, e.g. "url" or
   * "quantity unit".
   */
  def encodeText(what: String, value: String): String = {
    val index = loneSurrogateIndex(value)
    if (index >= 0)
      throw SchemaEncodeError(f"$what contains a lone surrogate U+${value.charAt(index).toInt}%04X at index $index")
    value
  }
}
