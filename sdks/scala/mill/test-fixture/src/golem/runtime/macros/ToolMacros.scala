package golem.runtime.macros

import golem.tool.{ExtendedToolType, ToolBuildError, ToolErrorSchema}

object ToolDefinitionMacro {
  def tryMetadata[A]: Either[ToolBuildError, ExtendedToolType] = ???
}

object ToolErrorSchemaDerivation {
  def derive[A]: ToolErrorSchema[A] = ???
}
