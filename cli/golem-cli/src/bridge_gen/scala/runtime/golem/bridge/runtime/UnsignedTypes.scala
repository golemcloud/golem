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
 * Unsigned integer wrappers matching the Golem Scala SDK's `UByte`, `UShort`,
 * `UInt` and `ULong`. Each holds the value in a wider signed Scala type (or a
 * `BigInt` for `ULong`) so the full unsigned range is representable.
 * Dependency-free.
 */
final case class UByte(value: Short)  extends AnyVal
final case class UShort(value: Int)   extends AnyVal
final case class UInt(value: Long)    extends AnyVal
final case class ULong(value: BigInt) extends AnyVal
