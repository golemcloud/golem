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

package golem.runtime.macros

import golem.config.ConfigSchema
import scala.quoted.*

private[macros] object ConfigMacro {
  def containsSecret(using Quotes)(tpe: quotes.reflect.TypeRepr, seen: Set[String] = Set.empty): Boolean = {
    import quotes.reflect.*
    val ty = tpe.dealias
    val id = ty.show
    if (ty.typeSymbol.fullName == "golem.config.Secret") true
    else if (seen(id)) false
    else {
      val next = seen + id
      ty.typeArgs.exists(containsSecret(_, next)) ||
      ty.typeSymbol.caseFields.exists(f => containsSecret(ty.memberType(f), next)) ||
      ty.typeSymbol.children.exists(c => containsSecret(if (c.isTerm) c.termRef else c.typeRef, next))
    }
  }

  def default(using Quotes)(owner: quotes.reflect.TypeRepr, field: quotes.reflect.Symbol): Option[String] = {
    import quotes.reflect.*
    val param       = owner.typeSymbol.primaryConstructor.paramSymss.flatten.find(_.name == field.name)
    val annotations = field.annotations ++ param.toList.flatMap(_.annotations)
    val values      = annotations
      .filter(_.tpe <:< TypeRepr.of[zio.blocks.schema.Modifier.config])
      .flatMap {
        case Apply(_, List(key, value)) if key.asExprOf[String].value.contains(ConfigSchema.DefaultKey) =>
          Some(value.asExprOf[String].valueOrAbort)
        case _ => None
      }
      .distinct
    if (values.size > 1) report.errorAndAbort(s"Duplicate config default on ${field.name}")
    values.headOption
  }

  def checkStaticFields(using
    Quotes
  )(tpe: quotes.reflect.TypeRepr, path: List[String], seen: Set[String] = Set.empty): Unit = {
    import quotes.reflect.*
    val ty = tpe.dealias
    if (!seen(ty.show)) {
      val next = seen + ty.show
      ty.typeSymbol.caseFields.foreach { field =>
        if (field.name.startsWith("$") || field.name.contains("."))
          report.errorAndAbort(s"Invalid config field ${field.name}")
        if (default(ty, field).nonEmpty)
          report.errorAndAbort(
            s"Place the config default on the owning static field ${path.mkString(".")}, not ${field.name}"
          )
        checkStaticFields(ty.memberType(field), path :+ field.name, next)
      }
      ty.typeArgs.foreach(checkStaticFields(_, path, next))
      ty.typeSymbol.children.foreach(c => checkStaticFields(if (c.isTerm) c.termRef else c.typeRef, path, next))
    }
  }
}
