/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.reflection

import golem.host.js.JsComponentId
import golem.runtime.tool.client.ToolRpcClient
import golem.runtime.tool.host.ToolHostApi
import golem.schema._
import golem.schema.SchemaTypeBody._
import golem.schema.SchemaValue._
import golem.schema.wire.SchemaWire
import golem.tool._
import golem.tool.wire._
import zio.blocks.schema.json.Json

import scala.concurrent.{ExecutionContext, Future}
import scala.util.control.NonFatal

private[reflection] object ToolReflectionFailures {
  private def protocol(error: Throwable): ToolError[Nothing] =
    ToolError.Rpc(RpcError.Protocol(Option(error.getMessage).getOrElse(error.toString)))

  def attempt[A](call: => Either[ToolError[NamedToolError], A]): Either[ToolError[NamedToolError], A] =
    try call
    catch {
      case NonFatal(error) =>
        Left(ToolError.Rpc(RpcError.Protocol(Option(error.getMessage).getOrElse(error.toString))))
    }

  def recover[A](future: Future[Either[ToolError[NamedToolError], A]])(implicit
    ec: ExecutionContext
  ): Future[Either[ToolError[NamedToolError], A]] =
    future.recover { case NonFatal(error) =>
      Left(ToolError.Rpc(RpcError.Protocol(Option(error.getMessage).getOrElse(error.toString))))
    }

  def collect[A](
    stdout: Option[ToolInputStream],
    result: Future[Either[ToolError[NamedToolError], A]]
  )(implicit ec: ExecutionContext): Future[Either[ToolError[NamedToolError], (A, Array[Byte])]] = {
    def drain(stream: ToolInputStream, chunks: Vector[Array[Byte]]): Future[Array[Byte]] =
      stream.read().flatMap {
        case Right(Some(bytes)) => drain(stream, chunks :+ bytes)
        case Right(None)        => Future.successful(chunks.flatten.toArray)
        case Left(failure)      => Future.failed(new ToolStreamException(failure))
      }
    val terminal = result.map(Right(_): Either[Throwable, Either[ToolError[NamedToolError], A]]).recover { case error =>
      Left(error)
    }
    val output = stdout
      .fold(Future.successful(Array.emptyByteArray))(drain(_, Vector.empty))
      .map(Right(_): Either[Throwable, Array[Byte]])
      .recover { case error => Left(error) }
    terminal.zip(output).map {
      case (Right(Left(error @ ToolError.Tool(_))), _) => Left(error)
      case (Left(error), _)                            => Left(protocol(error))
      case (_, Left(error))                            => Left(protocol(error))
      case (Right(value), Right(bytes))                => value.map(_ -> bytes)
    }
  }
}

/**
 * A selected argument in the canonical input record of a discovered command.
 */
final case class ToolArgument(
  kind: String,
  name: String,
  aliases: List[String],
  short: Option[Char],
  required: Boolean,
  default: Option[SchemaValue],
  schema: SchemaRef,
  optionalCarrier: Boolean
)

/** One immutable snapshot of an accessible host tool registration. */
final class ToolType private[reflection] (
  val lookupName: String,
  val definition: WitTool,
  val implementedBy: ComponentId
) {
  private val decoded = SchemaWire.schemaGraphRootsFromWit(definition.schema)
  private val graph   = decoded.graph

  val name: String    = definition.commands.nodes.head.name
  val version: String = definition.version

  private def schemaAt(index: Int): SchemaRef =
    SchemaRef(graph, decoded.at(index))

  private def optionSchema(shape: WitOptionShape): SchemaRef = shape match {
    case WitOptionShape.Scalar(index)         => schemaAt(index)
    case WitOptionShape.OptionalScalar(index) => schemaAt(index)
    case WitOptionShape.RepeatableList(item)  =>
      val inner = schemaAt(item.itemType)
      SchemaRef(inner.graph, SchemaType(ListType(inner.root)))
    case WitOptionShape.RepeatableMap(item) => schemaAt(item.mapType)
  }

  private def argument(option: WitOptionSpec): ToolArgument = {
    val repeatable = option.shape match {
      case WitOptionShape.RepeatableList(_) | WitOptionShape.RepeatableMap(_) => true
      case _                                                                  => false
    }
    ToolArgument(
      "option",
      option.long,
      option.aliases,
      option.short,
      option.required,
      option.default.map(SchemaWire.schemaValueFromWit),
      optionSchema(option.shape),
      !option.required && option.default.isEmpty && !repeatable
    )
  }

  private def argument(flag: FlagSpec): ToolArgument = {
    val root = flag.shape match {
      case FlagShape.BoolFlag(_)  => SchemaType(BoolType)
      case FlagShape.CountFlag(_) => SchemaType(U32Type(None))
    }
    ToolArgument(
      "flag",
      flag.long,
      flag.aliases,
      flag.short,
      required = false,
      Some(flag.shape match {
        case FlagShape.BoolFlag(shape) => BoolValue(shape.default)
        case FlagShape.CountFlag(_)    => U32Value(0)
      }),
      SchemaRef(SchemaGraph(graph.defs, root)),
      optionalCarrier = false
    )
  }

  /** The root path is empty; aliases resolve to the canonical path. */
  def command(path: List[String]): Either[GolemReflectError, ToolCommand] = {
    val nodes    = definition.commands.nodes
    val resolved = path.foldLeft(Option((0, List.empty[Int]))) { (current, segment) =>
      current.flatMap { case (index, indices) =>
        nodes(index).subcommands.find { child =>
          nodes.lift(child).exists(node => node.name == segment || node.aliases.contains(segment))
        }.map(child => (child, indices :+ child))
      }
    }
    resolved match {
      case Some((index, indices)) =>
        val body      = nodes(index).body
        val arguments = body.toList.flatMap { callable =>
          val localNames = (callable.positionals.fixed.map(_.name) ++ callable.positionals.tail.map(_.name) ++
            callable.options.flatMap(o => o.long :: o.aliases) ++ callable.flags.flatMap(f =>
              f.long :: f.aliases
            )).toSet
          val globals = (0 :: indices).flatMap { nodeIndex =>
            val node = nodes(nodeIndex)
            node.globals.options.map(argument) ++ node.globals.flags.map(argument)
          }.filterNot(arg => (arg.name :: arg.aliases).exists(localNames.contains))
          val positionals = callable.positionals.fixed.map { item =>
            ToolArgument(
              "positional",
              item.name,
              Nil,
              None,
              item.required,
              item.default.map(SchemaWire.schemaValueFromWit),
              schemaAt(item.tpe),
              !item.required && item.default.isEmpty
            )
          }
          val tail = callable.positionals.tail.toList.map { item =>
            val inner = schemaAt(item.itemType)
            ToolArgument(
              "tail",
              item.name,
              Nil,
              None,
              item.min > 0,
              None,
              SchemaRef(inner.graph, SchemaType(ListType(inner.root))),
              optionalCarrier = false
            )
          }
          globals ++ positionals ++ tail ++ callable.options.map(argument) ++ callable.flags.map(argument)
        }
        val inputRoot = record(arguments.map { arg =>
          arg.name -> (if (arg.optionalCarrier) SchemaType(OptionType(arg.schema.root)) else arg.schema.root)
        })
        val canonicalPath = indices.map(nodes(_).name)
        Right(
          new ToolCommand(
            this,
            canonicalPath,
            body,
            nodes(index).subcommands.map(nodes(_).name),
            arguments,
            SchemaRef(SchemaGraph(graph.defs, inputRoot)),
            SchemaGraph(graph.defs, inputRoot)
          )
        )
      case _ => Left(GolemReflectError.Discovery(s"Tool '$lookupName' has no command '${path.mkString(" ")}'"))
    }
  }

  private def record(fields: List[(String, SchemaType)]): SchemaType =
    SchemaType(RecordType(fields.map { case (name, root) => NamedFieldType(name, root, MetadataEnvelope.empty) }))

  def client: ReflectedToolClient = new ReflectedToolClient(this)

  private[reflection] def resultSchema(index: Int): SchemaRef = schemaAt(index)
}

object ToolType {
  private[reflection] def fromRegistered(raw: ToolHostApi.RegisteredTool): Either[GolemReflectError, ToolType] =
    try {
      if (raw.definition.commands.nodes.isEmpty)
        Left(GolemReflectError.Discovery(s"Tool '${raw.lookupName}' has no root command"))
      else Right(new ToolType(raw.lookupName, raw.definition, ComponentId.fromJs(raw.implementedBy)))
    } catch { case NonFatal(error) => Left(GolemReflectError.SchemaDecode(error.getMessage)) }
}

final class ReflectedToolClient private[reflection] (val tool: ToolType) {
  def command(path: List[String]): Either[GolemReflectError, ToolCommand] = tool.command(path)
}

/**
 * A caller-owned typed tool definition. The generated client may describe a
 * subset of the deployed tool; binding performs no discovery or compatibility
 * preflight.
 */
final class ToolClientDefinition[Client] private (
  val name: Option[String],
  createClient: String => Client
) {
  def client: Either[GolemReflectError, Client] =
    name match {
      case Some(lookupName) => create(lookupName)
      case None             => Left(GolemReflectError.Discovery("a nameless tool client definition requires a target name"))
    }

  def client(targetName: String): Either[GolemReflectError, Client] =
    create(name.getOrElse(targetName))

  private def create(lookupName: String): Either[GolemReflectError, Client] =
    if (lookupName.trim.isEmpty)
      Left(GolemReflectError.Discovery("tool client target name cannot be empty"))
    else
      try Right(createClient(lookupName))
      catch {
        case error: golem.runtime.tool.client.ToolRpcConstructionException =>
          Left(GolemReflectError.ToolRpc(error.failure))
        case NonFatal(error) =>
          Left(GolemReflectError.Discovery(Option(error.getMessage).getOrElse(error.toString)))
      }
}

object ToolClientDefinition {
  def named[Client](name: String)(createClient: String => Client): ToolClientDefinition[Client] =
    new ToolClientDefinition(Some(name), createClient)

  def unnamed[Client](createClient: String => Client): ToolClientDefinition[Client] =
    new ToolClientDefinition(None, createClient)
}

/**
 * A discovered command with a selected graph root for each declared surface.
 */
final class ToolCommand private[reflection] (
  val tool: ToolType,
  val path: List[String],
  val body: Option[WitCommandBody],
  val subcommands: List[String],
  val arguments: List[ToolArgument],
  val inputSchema: SchemaRef,
  private val wireInput: SchemaGraph
) {
  private implicit val ec: ExecutionContext = ToolInvokerRuntime.executionContext

  val result: Option[SchemaRef]                 = body.flatMap(_.result).map(spec => tool.resultSchema(spec.tpe))
  val errors: List[(String, Option[SchemaRef])] =
    body.toList.flatMap(_.errors).map(error => error.name -> error.payload.map(tool.resultSchema))

  def packJson(input: Json): Either[ToolError[Nothing], SchemaValue] =
    try
      inputSchema
        .packJson(input)
        .left
        .map(issue => ToolError.InvalidInput(issue.message))
        .flatMap(value =>
          inputSchema
            .validateValue(value)
            .left
            .map(issues => ToolError.InvalidInput(issues.map(_.message).mkString("; ")))
        )
        .flatMap(validateConstraints)
    catch { case NonFatal(error) => Left(ToolError.InvalidInput(Option(error.getMessage).getOrElse(error.toString))) }

  private def checkedInput(value: SchemaValue): Either[ToolError[Nothing], TypedSchemaValue] =
    inputSchema
      .validateValue(value)
      .left
      .map(issues => ToolError.InvalidInput(issues.map(_.message).mkString("; ")))
      .flatMap(validateConstraints)
      .map(value => TypedSchemaValue(wireInput, value))

  private def validateConstraints(value: SchemaValue): Either[ToolError[Nothing], SchemaValue] = {
    val values = value match {
      case RecordValue(fields) => arguments.map(_.name).zip(fields).toMap
      case _                   => return Left(ToolError.InvalidInput("tool input must be a record"))
    }
    def matches(reference: WitRef): Boolean = {
      val name = reference match {
        case WitRef.Present(name) => name
        case WitRef.ValueIs(item) => item.name
      }
      val argument = arguments.find(arg => arg.name == name || arg.aliases.contains(name)).get
      val actual   = values(argument.name)
      reference match {
        case WitRef.Present(_) =>
          if (argument.kind == "flag") argument.default.exists(_ != actual)
          else if (argument.default.contains(actual)) false
          else
            actual match {
              case OptionValue(value)     => value.nonEmpty
              case ListValue(values)      => values.nonEmpty
              case FixedListValue(values) => values.nonEmpty
              case MapValue(entries)      => entries.nonEmpty
              case BoolValue(value)       => value
              case U32Value(value)        => value != 0
              case _                      => true
            }
        case WitRef.ValueIs(item) =>
          val expected                                  = SchemaWire.schemaValueFromWit(item.value)
          def matchesValue(value: SchemaValue): Boolean =
            if (value == expected) true
            else
              value match {
                case OptionValue(inner)       => inner.exists(matchesValue)
                case ListValue(elements)      => elements.exists(matchesValue)
                case FixedListValue(elements) => elements.exists(matchesValue)
                case MapValue(entries)        => entries.exists(entry => matchesValue(entry.value))
                case _                        => false
              }
          matchesValue(actual)
      }
    }
    def quant(refs: List[WitRef], all: Boolean): Boolean = if (all) refs.forall(matches) else refs.exists(matches)
    val okay                                             = body.toList.flatMap(_.constraints).forall {
      case WitConstraint.RequiresAll(refs) => quant(refs, all = true)
      case WitConstraint.RequiresAny(refs) => quant(refs, all = false)
      case WitConstraint.AllOrNone(refs)   =>
        refs.count(matches) match {
          case 0     => true
          case count => count == refs.size
        }
      case WitConstraint.MutexGroups(groups) => groups.count(group => quant(group.refs, all = true)) <= 1
      case WitConstraint.Implies(item)       =>
        !quant(item.lhs, item.lhsQuant == Quantifier.All) || quant(item.rhs, item.rhsQuant == Quantifier.All)
      case WitConstraint.Forbids(item) =>
        !quant(item.lhs, item.lhsQuant == Quantifier.All) || !quant(item.rhs, all = false)
    }
    if (okay) Right(value) else Left(ToolError.InvalidInput("tool command constraints failed"))
  }

  private[reflection] def mapFailure(failure: ToolRpcFailure): ToolError[NamedToolError] = failure match {
    case ToolRpcFailure.ProtocolError(message)                                           => ToolError.Rpc(RpcError.Protocol(message))
    case ToolRpcFailure.Denied(message)                                                  => ToolError.Rpc(RpcError.Denied(message))
    case ToolRpcFailure.NotFound(message)                                                => ToolError.Rpc(RpcError.NotFound(message))
    case ToolRpcFailure.RemoteInternalError(message)                                     => ToolError.Rpc(RpcError.RemoteInternal(message))
    case ToolRpcFailure.Cancelled                                                        => ToolError.Rpc(RpcError.Cancelled)
    case ToolRpcFailure.ResourceExhausted(message)                                       => ToolError.Rpc(RpcError.ResourceExhausted(message))
    case ToolRpcFailure.RemoteToolError(ToolInvokeError.UnknownToolError(name, payload)) =>
      errors.find(_._1 == name) match {
        case None                                                => ToolError.UnknownToolError(name, payload)
        case Some((_, None)) if payload.value != TupleValue(Nil) =>
          ToolError.MalformedRemoteOutput(s"tool error '$name' has an unexpected payload")
        case Some((_, Some(schema)))
            if !ToolGraphs.schemaShapesMatch(payload.graph, SchemaGraph(schema.graph.defs, schema.root)) ||
              schema.validateValue(payload.value).isLeft =>
          ToolError.MalformedRemoteOutput(s"tool error '$name' has a malformed payload")
        case _ => ToolError.Tool(NamedToolError(name, payload))
      }
    case ToolRpcFailure.RemoteToolError(error) => ToolError.RemoteTool(error)
  }

  private[reflection] def decodeResult(value: ToolInvokeResult): Either[ToolError[Nothing], Option[SchemaValue]] =
    (result, value.result) match {
      case (None, None) => Right(None)
      case (Some(schema), Some(payload))
          if ToolGraphs.schemaShapesMatch(payload.graph, SchemaGraph(schema.graph.defs, schema.root)) =>
        schema
          .validateValue(payload.value)
          .left
          .map(issues => ToolError.MalformedRemoteOutput(issues.map(_.message).mkString("; ")))
          .map(Some(_))
      case _ => Left(ToolError.MalformedRemoteOutput("missing, unexpected, or mismatched structured result"))
    }

  def invokeValue(
    value: SchemaValue,
    stdin: Option[ToolInputStream] = None
  ): Future[Either[ToolError[NamedToolError], Option[SchemaValue]]] = {
    if (body.isEmpty)
      return Future.successful(Left(ToolError.InvalidInput("selected command is a namespace and cannot be invoked")))
    if (body.exists(_.stdout.exists(_.required)))
      return Future.successful(Left(ToolError.InvalidInput("command requires caller-readable stdout")))
    startValue(value, stdin) match {
      case Left(error)    => Future.successful(Left(error))
      case Right(started) => started.collect().map(_.map(_._1))
    }
  }

  def invokeJson(
    value: Json,
    stdin: Option[ToolInputStream] = None
  ): Future[Either[ToolError[NamedToolError], Option[Json]]] =
    packJson(value) match {
      case Left(error)   => Future.successful(Left(error))
      case Right(packed) =>
        invokeValue(packed, stdin).map(_.flatMap {
          case None         => Right(None)
          case Some(output) =>
            result.get.unpackJson(output).left.map(issue => ToolError.MalformedRemoteOutput(issue.message)).map(Some(_))
        })
    }

  def startValue(
    value: SchemaValue,
    stdin: Option[ToolInputStream] = None
  ): Either[ToolError[NamedToolError], ReflectedToolInvocation] = ToolReflectionFailures.attempt {
    if (body.isEmpty)
      Left(ToolError.InvalidInput("selected command is a namespace and cannot be invoked"))
    else if (body.exists(_.stdin.exists(_.required)) && stdin.isEmpty)
      Left(ToolError.InvalidInput("command requires stdin"))
    else
      for {
        input     <- checkedInput(value)
        transport <- ToolRpcClient.tryTransport(tool.lookupName).left.map(mapFailure)
        started   <- transport.start(path, input, stdin, body.exists(_.stdout.nonEmpty)).left.map(mapFailure)
      } yield ReflectedToolInvocation(
        started.stdout,
        ToolReflectionFailures.recover(started.result.map(_.left.map(mapFailure).flatMap(decodeResult))),
        started.cancel
      )
  }

  def startJson(
    value: Json,
    stdin: Option[ToolInputStream] = None
  ): Either[ToolError[NamedToolError], ReflectedToolJsonInvocation] =
    packJson(value).flatMap(startValue(_, stdin)).map { started =>
      ReflectedToolJsonInvocation(
        started.stdout,
        ToolReflectionFailures.recover(started.result.map(_.flatMap {
          case None         => Right(None)
          case Some(output) =>
            result.get.unpackJson(output).left.map(issue => ToolError.MalformedRemoteOutput(issue.message)).map(Some(_))
        })),
        started.cancel
      )
    }

  def triggerValue(
    value: SchemaValue,
    stdin: Option[ToolInputStream] = None
  ): Either[ToolError[NamedToolError], Unit] = ToolReflectionFailures.attempt {
    if (body.isEmpty)
      Left(ToolError.InvalidInput("selected command is a namespace and cannot be invoked"))
    else if (body.exists(_.stdout.exists(_.required)))
      Left(ToolError.InvalidInput("command requires caller-readable stdout"))
    else if (body.exists(_.stdin.exists(_.required)) && stdin.isEmpty)
      Left(ToolError.InvalidInput("command requires stdin"))
    else
      checkedInput(value).flatMap(input =>
        ToolRpcClient.trigger(tool.lookupName, path, input, stdin).left.map(mapFailure)
      )
  }

  def triggerJson(value: Json, stdin: Option[ToolInputStream] = None): Either[ToolError[NamedToolError], Unit] =
    packJson(value).flatMap(triggerValue(_, stdin))
}

final case class ReflectedToolInvocation(
  stdout: Option[ToolInputStream],
  result: Future[Either[ToolError[NamedToolError], Option[SchemaValue]]],
  cancel: () => Unit
) {
  def collect()(implicit
    ec: ExecutionContext
  ): Future[Either[ToolError[NamedToolError], (Option[SchemaValue], Array[Byte])]] =
    ToolReflectionFailures.collect(stdout, result)
}

final case class ReflectedToolJsonInvocation(
  stdout: Option[ToolInputStream],
  result: Future[Either[ToolError[NamedToolError], Option[Json]]],
  cancel: () => Unit
) {
  def collect()(implicit
    ec: ExecutionContext
  ): Future[Either[ToolError[NamedToolError], (Option[Json], Array[Byte])]] =
    ToolReflectionFailures.collect(stdout, result)
}

/** Fully dynamic tool calls accept only caller-packed values. */
final class DynamicToolClient(val toolName: String) {
  private implicit val ec: ExecutionContext = ToolInvokerRuntime.executionContext

  private def mapFailure(failure: ToolRpcFailure): ToolError[NamedToolError] = failure match {
    case ToolRpcFailure.ProtocolError(message)                                           => ToolError.Rpc(RpcError.Protocol(message))
    case ToolRpcFailure.Denied(message)                                                  => ToolError.Rpc(RpcError.Denied(message))
    case ToolRpcFailure.NotFound(message)                                                => ToolError.Rpc(RpcError.NotFound(message))
    case ToolRpcFailure.RemoteInternalError(message)                                     => ToolError.Rpc(RpcError.RemoteInternal(message))
    case ToolRpcFailure.Cancelled                                                        => ToolError.Rpc(RpcError.Cancelled)
    case ToolRpcFailure.ResourceExhausted(message)                                       => ToolError.Rpc(RpcError.ResourceExhausted(message))
    case ToolRpcFailure.RemoteToolError(ToolInvokeError.UnknownToolError(name, payload)) =>
      ToolError.Tool(NamedToolError(name, payload))
    case ToolRpcFailure.RemoteToolError(error) => ToolError.RemoteTool(error)
  }

  def invoke(
    path: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream] = None
  ): Future[Either[ToolError[NamedToolError], ToolInvokeResult]] =
    ToolReflectionFailures.attempt(ToolRpcClient.tryTransport(toolName).left.map(mapFailure)) match {
      case Left(failure)    => Future.successful(Left(failure))
      case Right(transport) =>
        ToolReflectionFailures.recover(
          ToolClientRuntime.invokeAndAwait(transport, path, input, stdin, error => Right(error))
        )
    }

  def start(
    path: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream] = None,
    stdout: Boolean = false
  ): Either[ToolError[NamedToolError], DynamicToolInvocation] =
    ToolReflectionFailures.attempt(
      for {
        transport <- ToolRpcClient.tryTransport(toolName).left.map(mapFailure)
        started   <- transport.start(path, input, stdin, stdout).left.map(mapFailure)
      } yield DynamicToolInvocation(
        started.stdout,
        ToolReflectionFailures.recover(started.result.map(_.left.map(mapFailure))),
        started.cancel
      )
    )

  def trigger(
    path: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream] = None
  ): Either[ToolError[NamedToolError], Unit] =
    ToolReflectionFailures.attempt(ToolRpcClient.trigger(toolName, path, input, stdin).left.map(mapFailure))
}

final case class DynamicToolInvocation(
  stdout: Option[ToolInputStream],
  result: Future[Either[ToolError[NamedToolError], ToolInvokeResult]],
  cancel: () => Unit
) {
  def collect()(implicit
    ec: ExecutionContext
  ): Future[Either[ToolError[NamedToolError], (ToolInvokeResult, Array[Byte])]] =
    ToolReflectionFailures.collect(stdout, result)
}
