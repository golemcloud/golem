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

package example.templates

import scala.scalajs.js
import scala.scalajs.js.annotation.JSGlobal
import scala.scalajs.js.typedarray.Uint8Array
import scala.annotation.unused

private[templates] object Utf8 {
  @js.native
  @JSGlobal("TextEncoder")
  private class TextEncoder() extends js.Object {
    def encode(input: String): Uint8Array = js.native
  }

  @js.native
  @JSGlobal("TextDecoder")
  private class TextDecoder(@unused label: String = "utf-8") extends js.Object {
    def decode(input: Uint8Array): String = js.native
  }

  def encode(input: String): Uint8Array =
    new TextEncoder().encode(input)

  def decode(bytes: Uint8Array): String =
    new TextDecoder("utf-8").decode(bytes)

  def encodeBytes(input: String): Array[Byte] = {
    val u8  = encode(input)
    val out = new Array[Byte](u8.length)
    var i   = 0
    while (i < u8.length) {
      out(i) = u8(i).toByte
      i += 1
    }
    out
  }

  def decodeBytes(bytes: Array[Byte]): String = {
    val u8 = new Uint8Array(bytes.length)
    var i  = 0
    while (i < bytes.length) {
      u8(i) = (bytes(i) & 0xff).toShort
      i += 1
    }
    decode(u8)
  }
}
