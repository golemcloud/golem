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

package java.security

import scala.scalajs.js
import scala.scalajs.js.typedarray.Int8Array

/**
 * `java.security.SecureRandom` for the Golem guest runtime.
 *
 * Bytes come from `globalThis.crypto.getRandomValues`, which the guest runtime
 * backs with `wasi:random`. The executor records `wasi:random` results in the
 * oplog, so the values are stable across replay.
 */
class SecureRandom extends java.util.Random(0L) {

  def this(seed: Array[Byte]) = this()

  override def setSeed(seed: Long): Unit = ()

  def setSeed(seed: Array[Byte]): Unit = ()

  def getAlgorithm(): String = "Golem"

  def generateSeed(numBytes: Int): Array[Byte] = {
    val bytes = new Array[Byte](numBytes)
    nextBytes(bytes)
    bytes
  }

  override def nextBytes(bytes: Array[Byte]): Unit = {
    val crypto = js.Dynamic.global.crypto
    var offset = 0
    while (offset < bytes.length) {
      val len    = Math.min(65536, bytes.length - offset)
      val buffer = new Int8Array(len)
      crypto.getRandomValues(buffer)
      var i = 0
      while (i < len) {
        bytes(offset + i) = buffer(i)
        i += 1
      }
      offset += len
    }
  }

  override protected def next(numBits: Int): Int = {
    val bytes = new Array[Byte](4)
    nextBytes(bytes)
    val value =
      ((bytes(0) & 0xff) << 24) | ((bytes(1) & 0xff) << 16) | ((bytes(2) & 0xff) << 8) | (bytes(3) & 0xff)
    if (numBits >= 32) value else value >>> (32 - numBits)
  }
}

object SecureRandom {
  def getInstance(algorithm: String): SecureRandom = new SecureRandom()

  def getInstanceStrong(): SecureRandom = new SecureRandom()
}
