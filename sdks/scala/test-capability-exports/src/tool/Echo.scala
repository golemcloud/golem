package capabilityfixture

import golem.runtime.annotations.{toolDefinition, toolImplementation}

@toolDefinition(name = "echo", version = "1.0.0")
trait Echo {
  def echo(value: String): String
}

@toolImplementation()
final class EchoImpl extends Echo {
  def echo(value: String): String = s"echo:$value"
}
