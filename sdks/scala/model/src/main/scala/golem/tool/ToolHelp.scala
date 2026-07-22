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

package golem.tool

/**
 * Help-text rendering for any node of a tool's command tree, projected purely
 * from the tool metadata.
 */
object ToolHelp {

  /**
   * Render help text for a command node addressed by `commandPath` (empty path =
   * root). Lists inherited globals, the body's positionals/options/flags, and
   * subcommands.
   */
  def renderHelp(
    tool: ExtendedToolType,
    commandPath: List[String]
  ): Either[ToolBuildError, String] =
    ToolValidation.resolveCommandPath(tool, commandPath).flatMap { idx =>
      tool.commands.lift(idx) match {
        case None    => Left(ToolBuildError.EmptyCommandTree)
        case Some(n) =>
          val out = new StringBuilder
          out ++= s"Usage: ${n.name}\n\n${n.doc.summary}\n${n.doc.description}\n"
          val globals = tool.effectiveGlobals(idx)
          if (globals.nonEmpty) {
            out ++= "\nGlobals:\n"
            globals.foreach {
              case EffectiveCommandField.OptionField(o) => out ++= s"  --${o.long}\t${o.doc.summary}\n"
              case EffectiveCommandField.FlagField(f)   => out ++= s"  --${f.long}\t${f.doc.summary}\n"
            }
          }
          n.body.foreach { b =>
            if (b.positionals.fixed.nonEmpty) {
              out ++= "\nPositionals:\n"
              b.positionals.fixed.foreach(p => out ++= s"  ${p.name}\t${p.doc.summary}\n")
            }
            b.positionals.tail.foreach { t =>
              out ++= "\nTail:\n"
              out ++= s"  ${t.name}...\t${t.doc.summary}\n"
            }
            if (b.options.nonEmpty) {
              out ++= "\nOptions:\n"
              b.options.foreach(o => out ++= s"  --${o.long}\t${o.doc.summary}\n")
            }
            if (b.flags.nonEmpty) {
              out ++= "\nFlags:\n"
              b.flags.foreach(f => out ++= s"  --${f.long}\t${f.doc.summary}\n")
            }
          }
          if (n.subcommands.nonEmpty) {
            out ++= "\nSubcommands:\n"
            n.subcommands.foreach { i =>
              tool.commands.lift(i).foreach(c => out ++= s"  ${c.name}\t${c.doc.summary}\n")
            }
          }
          Right(out.result())
      }
    }

  /**
   * Render help text for a single argument of the command addressed by
   * `commandPath`. Searches inherited globals, then the body's positionals,
   * tail, options, and flags (in canonical order), matching the long name or an
   * alias. Returns [[ToolBuildError.CommandNotFound]] if no such argument
   * exists on that command.
   */
  def renderArgumentHelp(
    tool: ExtendedToolType,
    commandPath: List[String],
    argName: String
  ): Either[ToolBuildError, String] =
    ToolValidation.resolveCommandPath(tool, commandPath).flatMap { idx =>
      val fromGlobals = tool.effectiveGlobals(idx).collectFirst {
        case EffectiveCommandField.OptionField(o) if o.long == argName || o.aliases.contains(argName) =>
          renderOptionHelp(o, global = true)
        case EffectiveCommandField.FlagField(f) if f.long == argName || f.aliases.contains(argName) =>
          renderFlagHelp(f, global = true)
      }

      def fromBody: Option[String] =
        tool.commands.lift(idx).flatMap(_.body).flatMap { body =>
          val positional = body.positionals.fixed.collectFirst {
            case p if p.name == argName =>
              val required = if (p.required) ", required" else ""
              s"${p.name} (positional$required)\n${p.doc.summary}\n${p.doc.description}\n"
          }
          def tail: Option[String] = body.positionals.tail.collect {
            case t if t.name == argName =>
              s"${t.name}... (tail positional)\n${t.doc.summary}\n${t.doc.description}\n"
          }
          def option: Option[String] = body.options.collectFirst {
            case o if o.long == argName || o.aliases.contains(argName) =>
              renderOptionHelp(o, global = false)
          }
          def flag: Option[String] = body.flags.collectFirst {
            case f if f.long == argName || f.aliases.contains(argName) =>
              renderFlagHelp(f, global = false)
          }
          positional.orElse(tail).orElse(option).orElse(flag)
        }

      fromGlobals.orElse(fromBody) match {
        case Some(text) => Right(text)
        case None       => Left(ToolBuildError.CommandNotFound(argName))
      }
    }

  private def renderOptionHelp(o: ExtendedOptionSpec, global: Boolean): String = {
    val suffix = if (global) ", global" else ""
    s"--${o.long} (option$suffix)\n${o.doc.summary}\n${o.doc.description}\n"
  }

  private def renderFlagHelp(f: FlagSpec, global: Boolean): String = {
    val suffix = if (global) ", global" else ""
    s"--${f.long} (flag$suffix)\n${f.doc.summary}\n${f.doc.description}\n"
  }
}
