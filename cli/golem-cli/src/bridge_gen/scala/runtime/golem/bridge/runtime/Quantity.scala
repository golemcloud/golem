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
 * A fixed-point quantity, `mantissa * 10^(-scale)` in `unit`. The marker type
 * `U` names the units the schema allows through its implicit [[QuantityUnit]].
 *
 * Mirrors the Golem Scala SDK's `golem.schema.Quantity`.
 */
final case class Quantity[U](mantissa: Long, scale: Int, unit: String)

/**
 * The units of the quantity marker `U`: only the base unit when there are no
 * suffixes, otherwise only the suffixes.
 */
trait QuantityUnit[U] {
  def baseUnit: String
  def allowedSuffixes: List[String]

  final def allows(unit: String): Boolean =
    if (allowedSuffixes.isEmpty) unit == baseUnit else allowedSuffixes.contains(unit)
}

object Quantity {
  def toSchemaValue[U](value: Quantity[U])(implicit unit: QuantityUnit[U]): SchemaValue = {
    if (!unit.allows(value.unit)) throw BridgeException(notAllowed(value.unit, unit))
    SchemaValue.QuantityValue(value.mantissa, value.scale, value.unit)
  }

  def fromSchemaValue[U](value: SchemaValue)(implicit unit: QuantityUnit[U]): Quantity[U] = value match {
    case SchemaValue.QuantityValue(mantissa, scale, u) if unit.allows(u) => Quantity[U](mantissa, scale, u)
    case SchemaValue.QuantityValue(_, _, u)                              => throw BridgeException(notAllowed(u, unit))
    case other                                                           => throw BridgeException(s"Expected quantity value, got $other")
  }

  private def notAllowed(value: String, unit: QuantityUnit[?]): String = {
    val allowed = if (unit.allowedSuffixes.isEmpty) List(unit.baseUnit) else unit.allowedSuffixes
    s"Quantity unit `$value` is not allowed. Allowed units: ${allowed.mkString(", ")}"
  }
}
