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

package golem.runtime.annotations

/**
 * Declares the units of a quantity marker type. Every listed unit is allowed
 * and the first one is the base unit; a single unit is the base unit with no
 * suffixes.
 *
 * {{{
 *   @quantityUnit("m", "cm", "km") sealed trait Length  // base m, suffixes m, cm, km
 *   @quantityUnit("kg") sealed trait Kilograms          // base kg, no suffixes
 * }}}
 *
 * The named `suffixes` argument sets the suffix list exactly, for a spec whose
 * suffixes do not start with the base unit. A quantity with suffixes accepts
 * only the suffixes, so `@quantityUnit("m", suffixes = Seq("cm", "km"))` does
 * not accept `m`.
 */
final class quantityUnit(baseUnit: String, suffixes: Seq[String]) extends scala.annotation.StaticAnnotation {
  def this(units: String*) = this(units.headOption.getOrElse(""), Nil)
}
