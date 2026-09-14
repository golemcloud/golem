package golem.runtime.autowire

import golem.tool.RawToolUnderlying

object ToolMiddlewareImplementation {
  def registerTransparent[Presented, Underlying, Surface, Impl](
    fromRaw: RawToolUnderlying => Underlying
  ): Unit = ()

  def registerAdapter[Presented, Expected, Underlying, Surface, Impl](
    fromRaw: RawToolUnderlying => Underlying
  ): Unit = ()

  def registerUniversal[Impl]: Unit = ()
}
