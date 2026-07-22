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

import scala.collection.mutable

/**
 * Mutable build context threaded through macro-generated tool descriptors.
 * Tracks the descriptor recursion stack (for subtree cycle detection), the
 * command path, the inherited globals in scope, and any pending graft-root
 * rename.
 */
final class ToolBuildCtx {
  private val stack               = mutable.ArrayBuffer.empty[String]
  private val commandPathBuf      = mutable.ArrayBuffer.empty[String]
  private val inheritedGlobalsBuf = mutable.ArrayBuffer.empty[EffectiveCommandField]
  private val graftRoots          = mutable.ArrayBuffer.empty[ToolBuildCtx.PendingGraftRoot]

  def pushDescriptor(identity: String): Either[ToolBuildError, Unit] =
    if (stack.contains(identity)) Left(ToolBuildError.SubtreeCycle(cyclePath(identity)))
    else {
      stack += identity
      Right(())
    }

  def popDescriptor(): Unit =
    if (stack.nonEmpty) stack.remove(stack.length - 1)

  /**
   * True when no ancestor descriptor is currently on the recursion stack — i.e.
   * this is the outermost descriptor invocation. Must be called from inside
   * [[withDescriptor]] (the current descriptor's identity is then on top of the
   * stack), so a value of `1` means there is exactly one descriptor in flight:
   * this one.
   *
   * A nested subtree child descriptor (called from a parent's subtree link)
   * returns `false`. The child therefore skips composition/normalization and
   * returns its raw command tree, with `value-is` literals still deferred; the
   * outermost descriptor normalizes the fully grafted tree once, when all
   * ancestor subtree globals and inherited-global de-projections are in scope.
   */
  def isOutermostDescriptor: Boolean = stack.length == 1

  /**
   * Build a child descriptor while the given identity is pushed on the
   * recursion stack, always popping afterwards, so an early error inside `f`
   * cannot leak a stack entry and falsely report a later cycle.
   */
  def withDescriptor[T](identity: String)(
    f: ToolBuildCtx => Either[ToolBuildError, T]
  ): Either[ToolBuildError, T] =
    pushDescriptor(identity).flatMap { _ =>
      try f(this)
      finally popDescriptor()
    }

  def inheritedGlobals: List[EffectiveCommandField] = inheritedGlobalsBuf.toList

  def withInheritedGlobals[T](globals: List[EffectiveCommandField])(
    f: ToolBuildCtx => Either[ToolBuildError, T]
  ): Either[ToolBuildError, T] = {
    val oldLen = inheritedGlobalsBuf.length
    inheritedGlobalsBuf ++= globals
    try f(this)
    finally inheritedGlobalsBuf.remove(oldLen, inheritedGlobalsBuf.length - oldLen)
  }

  def withGraftRoot[T](expectedName: String, overrideName: Option[String])(
    f: ToolBuildCtx => Either[ToolBuildError, T]
  ): Either[ToolBuildError, T] = {
    graftRoots += ToolBuildCtx.PendingGraftRoot(expectedName, overrideName)
    try f(this)
    finally graftRoots.remove(graftRoots.length - 1)
  }

  /**
   * Apply the innermost pending graft-root rename to `root`: without an
   * explicit override the root name must equal the expected (parent subtree
   * method) name; with one, the root is renamed.
   */
  def applyPendingGraftRoot(
    root: ExtendedCommandNode
  ): Either[ToolBuildError, ExtendedCommandNode] =
    graftRoots.lastOption match {
      case None          => Right(root)
      case Some(pending) =>
        pending.overrideName match {
          case None if root.name != pending.expectedName =>
            Left(ToolBuildError.SubtreeRootNameMismatch(pending.expectedName, root.name))
          case None       => Right(root)
          case Some(name) => Right(root.copy(name = name))
        }
    }

  private def cyclePath(repeated: String): String =
    (stack.toList :+ repeated).mkString(" -> ")

  def pushCommand(name: String): Unit = commandPathBuf += name

  def popCommand(): Unit =
    if (commandPathBuf.nonEmpty) commandPathBuf.remove(commandPathBuf.length - 1)

  def commandPath: List[String] = commandPathBuf.toList
}

object ToolBuildCtx {
  private final case class PendingGraftRoot(expectedName: String, overrideName: Option[String])
}

/** Implemented by macro-generated tool descriptor companions. */
trait ToolDefinitionDescriptor {
  def metadata(ctx: ToolBuildCtx): Either[ToolBuildError, ExtendedToolType]
}
