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

import scala.quoted.*

/**
 * The element types of a tuple type. A tuple of more than 22 elements has no
 * `TupleN` class and is the `h *: t` chain itself, so its elements are not its
 * type arguments or case fields.
 */
private[golem] object TupleElements {

  def of(using Quotes)(tpe: quotes.reflect.TypeRepr): List[quotes.reflect.TypeRepr] = {
    import quotes.reflect.*

    def loop(tpe: TypeRepr): List[TypeRepr] = tpe.dealias match {
      case AppliedType(tycon, List(head, tail)) if tycon.typeSymbol == TypeRepr.of[*:[Any, Tuple]].typeSymbol =>
        head :: loop(tail)
      case other if other =:= TypeRepr.of[EmptyTuple]   => Nil
      case other if defn.isTupleClass(other.typeSymbol) => other.typeArgs
      case other                                        => report.errorAndAbort(s"Cannot read the element types of tuple ${other.show}")
    }

    loop(tpe)
  }
}
