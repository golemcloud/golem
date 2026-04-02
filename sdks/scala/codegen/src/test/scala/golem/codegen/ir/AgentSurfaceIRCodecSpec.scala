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

class AgentSurfaceIRCodecSpec extends munit.FunSuite {

  private val sampleModule = Module(
    agents = List(
      AgentSurface(
        traitFqn = "example.CounterAgent",
        packageName = "example",
        simpleName = "CounterAgent",
        typeName = "CounterAgent",
        constructor = ConstructorSurface(
          params = List(ParamSurface("value", "String"))
        ),
        metadata = AgentMetadataSurface(
          description = Some("A simple counter."),
          mode = "durable",
          snapshotting = "disabled"
        ),
        methods = List(
          MethodSurface(
            name = "increment",
            params = List(ParamSurface("amount", "Int")),
            returnTypeExpr = "Int",
            principalParams = List(false)
          )
        )
      ),
      AgentSurface(
        traitFqn = "example.Shard",
        packageName = "example",
        simpleName = "Shard",
        typeName = "Shard",
        constructor = ConstructorSurface(
          params = List(
            ParamSurface("tableName", "String"),
            ParamSurface("shardId", "Int")
          )
        ),
        metadata = AgentMetadataSurface(
          description = None,
          mode = "durable",
          snapshotting = "every(1)"
        ),
        methods = Nil
      )
    )
  )

  test("roundtrip encode/decode") {
    val json   = AgentSurfaceIRCodec.encode(sampleModule)
    val result = AgentSurfaceIRCodec.decode(json)
    assertEquals(result, Right(sampleModule))
  }

  test("encode produces valid JSON with expected keys") {
    val json = AgentSurfaceIRCodec.encode(sampleModule)
    assert(json.contains("\"traitFqn\":\"example.CounterAgent\""))
    assert(json.contains("\"description\":\"A simple counter.\""))
    assert(json.contains("\"description\":null"))
    assert(json.contains("\"snapshotting\":\"every(1)\""))
  }

  test("decode rejects invalid JSON") {
    val result = AgentSurfaceIRCodec.decode("not json at all")
    assert(result.isLeft)
  }

  test("roundtrip with empty agents list") {
    val emptyModule = Module(agents = Nil)
    val json        = AgentSurfaceIRCodec.encode(emptyModule)
    val result      = AgentSurfaceIRCodec.decode(json)
    assertEquals(result, Right(emptyModule))
  }

  test("roundtrip with empty constructor params") {
    val module = Module(
      agents = List(
        AgentSurface(
          traitFqn = "example.NoCtorAgent",
          packageName = "example",
          simpleName = "NoCtorAgent",
          typeName = "NoCtorAgent",
          constructor = ConstructorSurface(params = Nil),
          metadata = AgentMetadataSurface(
            description = None,
            mode = "ephemeral",
            snapshotting = "disabled"
          ),
          methods = Nil
        )
      )
    )
    val json   = AgentSurfaceIRCodec.encode(module)
    val result = AgentSurfaceIRCodec.decode(json)
    assertEquals(result, Right(module))
  }

  test("handles special characters in strings") {
    val module = Module(
      agents = List(
        AgentSurface(
          traitFqn = "example.Agent",
          packageName = "example",
          simpleName = "Agent",
          typeName = "Agent",
          constructor = ConstructorSurface(params = Nil),
          metadata = AgentMetadataSurface(
            description = Some("A description with \"quotes\" and\nnewlines and \\ backslashes."),
            mode = "durable",
            snapshotting = "disabled"
          ),
          methods = Nil
        )
      )
    )
    val json   = AgentSurfaceIRCodec.encode(module)
    val result = AgentSurfaceIRCodec.decode(json)
    assertEquals(result, Right(module))
  }

  test("decode handles whitespace in JSON") {
    val json =
      """  {  "agents" : [  ]  }  """
    val result = AgentSurfaceIRCodec.decode(json)
    assertEquals(result, Right(Module(agents = Nil)))
  }

  test("roundtrip with methods") {
    val module = Module(
      agents = List(
        AgentSurface(
          traitFqn = "example.RpcAgent",
          packageName = "example",
          simpleName = "RpcAgent",
          typeName = "RpcAgent",
          constructor = ConstructorSurface(params = Nil),
          metadata = AgentMetadataSurface(
            description = None,
            mode = "durable",
            snapshotting = "disabled"
          ),
          methods = List(
            MethodSurface(
              name = "greet",
              params = List(ParamSurface("name", "String"), ParamSurface("age", "Int")),
              returnTypeExpr = "String",
              principalParams = List(false, false)
            ),
            MethodSurface(
              name = "reset",
              params = Nil,
              returnTypeExpr = "Unit",
              principalParams = Nil
            )
          )
        )
      )
    )
    val json   = AgentSurfaceIRCodec.encode(module)
    val result = AgentSurfaceIRCodec.decode(json)
    assertEquals(result, Right(module))
  }

  test("decode backward compat - missing methods field defaults to Nil") {
    val json =
      """|{
         |  "agents": [{
         |    "traitFqn": "example.Old",
         |    "packageName": "example",
         |    "simpleName": "Old",
         |    "typeName": "Old",
         |    "constructor": { "params": [] },
         |    "metadata": { "description": null, "mode": "durable", "snapshotting": "disabled" }
         |  }]
         |}""".stripMargin
    val result = AgentSurfaceIRCodec.decode(json)
    assert(result.isRight)
    assertEquals(result.toOption.get.agents.head.methods, Nil)
  }
}
