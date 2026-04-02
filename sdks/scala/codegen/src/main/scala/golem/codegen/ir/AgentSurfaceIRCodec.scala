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

package golem.codegen.ir

import AgentSurfaceIR._

object AgentSurfaceIRCodec {

  def encode(module: Module): String =
    moduleToJson(module).render()

  def decode(json: String): Either[String, Module] =
    try Right(moduleFromJson(ujson.read(json)))
    catch { case e: Exception => Left(e.getMessage) }

  private def moduleToJson(m: Module): ujson.Value =
    ujson.Obj("agents" -> ujson.Arr.from(m.agents.map(agentToJson)))

  private def agentToJson(a: AgentSurface): ujson.Value =
    ujson.Obj(
      "traitFqn"    -> a.traitFqn,
      "packageName" -> a.packageName,
      "simpleName"  -> a.simpleName,
      "typeName"    -> a.typeName,
      "constructor" -> ujson.Obj(
        "params" -> ujson.Arr.from(a.constructor.params.map(p => ujson.Obj("name" -> p.name, "typeExpr" -> p.typeExpr)))
      ),
      "metadata" -> ujson.Obj(
        "description"  -> a.metadata.description.fold[ujson.Value](ujson.Null)(ujson.Str(_)),
        "mode"         -> a.metadata.mode,
        "snapshotting" -> a.metadata.snapshotting
      ),
      "methods"      -> ujson.Arr.from(a.methods.map(methodToJson)),
      "configFields" -> ujson.Arr.from(a.configFields.map(configFieldToJson))
    )

  private def methodToJson(m: MethodSurface): ujson.Value =
    ujson.Obj(
      "name"            -> m.name,
      "params"          -> ujson.Arr.from(m.params.map(p => ujson.Obj("name" -> p.name, "typeExpr" -> p.typeExpr))),
      "returnTypeExpr"  -> m.returnTypeExpr,
      "principalParams" -> ujson.Arr.from(m.principalParams.map(ujson.Bool(_)))
    )

  private def configFieldToJson(cf: ConfigFieldSurface): ujson.Value =
    ujson.Obj(
      "path"     -> ujson.Arr.from(cf.path.map(ujson.Str(_))),
      "typeExpr" -> cf.typeExpr
    )

  private def moduleFromJson(v: ujson.Value): Module =
    Module(agents = v("agents").arr.toList.map(agentFromJson))

  private def agentFromJson(v: ujson.Value): AgentSurface = {
    val obj = v.obj
    AgentSurface(
      traitFqn = obj("traitFqn").str,
      packageName = obj("packageName").str,
      simpleName = obj("simpleName").str,
      typeName = obj("typeName").str,
      constructor = ConstructorSurface(
        params = obj("constructor")("params").arr.toList.map { p =>
          ParamSurface(name = p("name").str, typeExpr = p("typeExpr").str)
        }
      ),
      metadata = {
        val mo = obj("metadata").obj
        AgentMetadataSurface(
          description = mo("description") match {
            case ujson.Null => None
            case s          => Some(s.str)
          },
          mode = mo("mode").str,
          snapshotting = mo("snapshotting").str
        )
      },
      methods = obj.get("methods").map(_.arr.toList.map(methodFromJson)).getOrElse(Nil),
      configFields = obj.get("configFields").map(_.arr.toList.map(configFieldFromJson)).getOrElse(Nil)
    )
  }

  private def configFieldFromJson(v: ujson.Value): ConfigFieldSurface =
    ConfigFieldSurface(
      path = v("path").arr.toList.map(_.str),
      typeExpr = v("typeExpr").str
    )

  private def methodFromJson(v: ujson.Value): MethodSurface = {
    val params = v("params").arr.toList.map(p => ParamSurface(name = p("name").str, typeExpr = p("typeExpr").str))
    MethodSurface(
      name = v("name").str,
      params = params,
      returnTypeExpr = v("returnTypeExpr").str,
      principalParams = v.obj
        .get("principalParams")
        .map(_.arr.toList.map(_.bool))
        .getOrElse(List.fill(params.length)(false))
    )
  }
}
