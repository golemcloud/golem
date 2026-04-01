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

package golem.codegen.pipeline

import golem.codegen.discovery.SourceDiscovery

class CodegenPipelineSpec extends munit.FunSuite {

  private val agentSource = SourceDiscovery.SourceInput(
    "Counter.scala",
    """|package example
       |
       |@agentDefinition("counter-agent")
       |trait CounterAgent {
       |  class Id(val value: String)
       |  def increment(amount: Int): Int
       |}
       |
       |@agentImplementation()
       |final class CounterAgentImpl(private val value: String) extends CounterAgent {
       |  def increment(amount: Int): Int = amount
       |}
       |""".stripMargin
  )

  private def discover(sources: SourceDiscovery.SourceInput*): SourceDiscovery.Result =
    SourceDiscovery.discover(sources)

  test("pipeline with both auto-register and rpc enabled") {
    val discovered = discover(agentSource)
    val result     = CodegenPipeline.run(discovered, Some("example"), rpcEnabled = true)

    assert(result.autoRegister.isDefined)
    assert(result.autoRegister.get.files.nonEmpty)
    assert(result.autoRegister.get.implCount == 1)

    assert(result.rpc.files.nonEmpty)
    assertEquals(result.rpc.files.size, 1)
    assert(result.rpc.files.head.content.contains("CounterAgentClient"))
  }

  test("pipeline with only auto-register") {
    val discovered = discover(agentSource)
    val result     = CodegenPipeline.run(discovered, Some("example"), rpcEnabled = false)

    assert(result.autoRegister.isDefined)
    assert(result.rpc.files.isEmpty)
  }

  test("pipeline with only rpc enabled") {
    val discovered = discover(agentSource)
    val result     = CodegenPipeline.run(discovered, None, rpcEnabled = true)

    assert(result.autoRegister.isEmpty)
    assert(result.rpc.files.nonEmpty)
  }

  test("pipeline with nothing enabled") {
    val discovered = discover(agentSource)
    val result     = CodegenPipeline.run(discovered, None, rpcEnabled = false)

    assert(result.autoRegister.isEmpty)
    assert(result.rpc.files.isEmpty)
  }

  test("pipeline converts discovery methods to IR with principalParams") {
    val source = SourceDiscovery.SourceInput(
      "Agent.scala",
      """|package example
         |
         |@agentDefinition()
         |trait MyAgent {
         |  class Id()
         |  def process(caller: Principal, data: String): String
         |}
         |""".stripMargin
    )

    val discovered = discover(source)
    val result     = CodegenPipeline.run(discovered, None, rpcEnabled = true)

    assert(result.rpc.files.nonEmpty)
    val content = result.rpc.files.head.content
    // Principal param should be filtered out
    assert(content.contains("def apply(data: String)"), s"missing filtered apply in:\n$content")
    assert(!content.contains("caller: Principal"), s"principal param should be filtered:\n$content")
  }

  test("pipeline converts mode to IR") {
    val source = SourceDiscovery.SourceInput(
      "Eph.scala",
      """|package example
         |
         |@agentDefinition(mode = DurabilityMode.Ephemeral)
         |trait EphAgent {
         |  class Id()
         |  def hello(): String
         |}
         |""".stripMargin
    )

    val discovered = discover(source)
    val result     = CodegenPipeline.run(discovered, None, rpcEnabled = true)

    assert(result.rpc.files.nonEmpty)
    val content = result.rpc.files.head.content
    assert(content.contains("getPhantom"), s"ephemeral agent should have getPhantom:\n$content")
    assert(!content.contains("def get("), s"ephemeral agent should not have get:\n$content")
  }
}
