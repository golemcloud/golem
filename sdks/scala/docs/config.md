# Static config

An agent declares one config record with `AgentConfig[C]`. Its implementation
receives `Config[C]` and reads typed fields with `config.<field>`. `config.value`
returns the complete record. Each access reads the host captures for the current
owner revision. Values saved by application code, such as a constructor field,
remain application state; they do not become live config accessors. Secret
handles retain their own read behavior.

```scala
import golem.config.{AgentConfig, Config}
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import zio.blocks.schema.{Modifier, Schema}
import scala.concurrent.Future

final case class Settings(label: String, count: golem.UInt) derives Schema
final case class SupportConfig(
  @Modifier.config("golem.config.default", "4000") maxChars: Int,
  @Modifier.config("golem.config.default", "{\"label\":\"code-default\",\"count\":31}") settings: Settings,
  @Modifier.config("golem.config.default", "[null,7,\"retained\"]") slots: (Option[Int], Option[Int], String)
) derives Schema

@agentDefinition()
trait Support extends AgentConfig[SupportConfig] {
  class Id(val name: String)
  def limit(): Future[Int]
}

@agentImplementation()
final class SupportImpl(name: String, config: Config[SupportConfig]) extends Support {
  private val maxChars: Int = config.maxChars
  def limit(): Future[Int] = Future.successful(maxChars)
}
```

`Modifier.config("golem.config.default", json)` attaches canonical schema JSON to
a declared static field. Metadata production parses and validates the value,
then exports it as `defaultValue`. It accepts data, not a factory callback or a
managed handle. Ordinary Scala constructor defaults are not config defaults.

The host fills only missing static settings from code defaults, below deployment
layers and creation overrides. Explicit `null`, `false`, zero, empty strings and
empty lists are supplied values. A supplied invalid value or failed host read
does not select a code default. Defaults do not initialize Secrets or resources.

Plain record fields stay whole, including recursive records. A required record
that groups Secret handles is expanded into its independent fields. Put a
record default on its owning static field; annotations inside that static value
are rejected rather than discarded. Config field names must not start with `$`
or contain literal dots. Metadata, host reads and creation overrides project native
config fields to lowerCamelCase, for example `max_chars` to `maxChars`, and reject
collisions. Native accessors and ordinary method schemas retain their names.
Catalog paths such as `limits.support_default` remain exact values; runtime lookup
does not recase them.

Options and lists retain their schema shape. `[null,7,"retained"]` has three
tuple positions, not two. An absent default is different from the explicit JSON
default `"null"`. Metadata export validates defaults against both the config
schema and the native Scala type, including numeric values inside containers.

## sbt and Mill

Both plugins use the same source discovery and RPC generator. Static records
produce one typed creation-override parameter; required groups containing
Secrets expose only their static fields. The generated `create` method accepts
these optional overrides and passes them to the host when it creates the agent.

The real consumer in `test-static-config/` is shared by the sbt
`staticConfigFixture` project and the Mill `staticConfigFixture` module. It reads
config during construction and again in `read()`, so the same guest can exercise
default resolution, deployment values, creation overrides and reconstruction.
Publish the current SDK locally with `sbt golemPublishLocal` before building the
Mill consumer. Use Scala 3.8.2 for this consumer; the existing lightweight Mill
fixtures still compile with Scala 3.3.7.

```sh
# From sdks/scala
sbt "++3.8.2; staticConfigFixture/golemBuildComponent scala:static-config /absolute/path/static-config.js"
# From sdks/scala/mill, after local SDK publication
mill staticConfigFixture.golemBuildComponent --component scala:static-config --outPath /absolute/path/static-config-mill.js
```

These tasks build the JavaScript module and prepare the embedded QuickJS runtime.
The maintained `test-components/static-config-scala/golem.yaml` manifest also
injects and preinitializes the module, then stages the worker-executor fixture:

```sh
# From test-components/static-config-scala, with the current repository CLI
golem build --yes -P release
golem exec --yes -P release copy
```

The Scala group in `test-components/build-components.sh` builds and stages this
fixture. A linked module or a validated WASM does not establish successful hosted
execution.
