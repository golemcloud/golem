// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.http

import golem.runtime._
import golem.schema._
import golem.schema.SchemaTypeBody._
import golem.schema.validation.RefResolution

/**
 * Validation shared by generated declarations and the metadata extraction
 * boundary.
 */
object HttpAgentValidation {
  def validate(agent: AgentMetadata): Either[String, Unit] = {
    if (agent.methods.map(_.name).distinct.size != agent.methods.size) return Left("duplicate-method")
    if (
      agent.httpMount.exists(m =>
        !FileMappingParser.validateCompiled(m.staticBindings) ||
          !FileMappingParser.validateCompiled(m.filesystemBindings)
      )
    ) return Left("invalid-file-mapping")
    if (agent.kind == AgentTypeKind.HttpRouter) validateRouter(agent)
    else if (agent.methods.exists(_.httpEndpoints.exists(_.httpMethod == HttpMethod.Any)))
      Left("router-method-on-regular-agent")
    else
      agent.httpMount match {
        case Some(mount) if mount.staticBindings.nonEmpty        => Left("static-bindings-on-regular-agent")
        case Some(mount) if mount.openapiProviderMethod.nonEmpty => Left("openapi-provider-on-regular-agent")
        case Some(mount) if mount.filesystemBindings.nonEmpty    => validateFilesystem(agent, mount)
        case _                                                   => Right(())
      }
  }

  def checked(agent: AgentMetadata): AgentMetadata = {
    validate(agent).fold(error => throw new IllegalArgumentException(s"${agent.name}: $error"), identity)
    agent
  }

  private def validateRouter(agent: AgentMetadata): Either[String, Unit] = {
    if (!agent.mode.contains("ephemeral")) return Left("router-mode")
    if (agent.constructor.input.parameters.nonEmpty) return Left("router-constructor")
    if (agent.snapshotting != Snapshotting.Disabled) return Left("router-snapshot")
    val mount = agent.httpMount match {
      case Some(m) if m.pathPrefix.forall {
            case PathSegment.Literal(s) => FileMappingParser.validSegment(s)
            case _                      => false
          } =>
        m
      case _ => return Left("router-mount")
    }
    if (mount.filesystemBindings.nonEmpty) return Left("filesystem-owner")
    val provider = mount.openapiProviderMethod.flatMap(name => agent.methods.find(_.name == name))
    if (provider.map(_.name) != mount.openapiProviderMethod) return Left("router-method-role")
    provider match {
      case Some(p) if p.httpEndpoints.nonEmpty                                   => return Left("router-method-role")
      case Some(p) if p.input.parameters.nonEmpty || !outputMatches(p, t.string) => return Left("provider-schema")
      case _                                                                     => ()
    }
    val handlers = agent.methods.filterNot(m => mount.openapiProviderMethod.contains(m.name))
    if (handlers.size > 1) return Left("router-method-role")
    handlers.headOption match {
      case None    => Right(())
      case Some(h) =>
        h.httpEndpoints match {
          case List(endpoint) if endpoint.httpMethod == HttpMethod.Any && endpoint.pathSuffix.isEmpty =>
            if (
              endpoint.authOverride.nonEmpty || endpoint.corsOverride.nonEmpty || endpoint.headerVars.nonEmpty || endpoint.queryVars.nonEmpty
            )
              Left("handler-endpoint-policy")
            else
              h.input.userSupplied match {
                case List(request)
                    if request.name == "request" && matches(
                      request.graph,
                      request.graph.root,
                      HttpExchangeCodec.requestGraph.root
                    ) &&
                      outputMatches(h, HttpExchangeCodec.responseGraph.root) =>
                  Right(())
                case _ => Left("handler-schema")
              }
          case _ => Left("router-method-role")
        }
    }
  }

  private def outputMatches(method: MethodMetadata, expected: SchemaType): Boolean = method.output match {
    case OutputMetadata.Single(graph) => matches(graph, graph.root, expected)
    case _                            => false
  }

  // The canonical HTTP schemas are finite trees. Resolve references at every level;
  // nominal names and documentation do not affect structural equality.
  private def matches(graph: SchemaGraph, actual: SchemaType, expected: SchemaType): Boolean =
    RefResolution.resolveRef(graph, actual).toOption.exists { resolved =>
      (resolved.body, expected.body) match {
        case (RecordType(a), RecordType(e)) =>
          a.size == e.size && a.zip(e).forall { case (x, y) =>
            x.name == y.name && matches(graph, x.body, y.body)
          }
        case (ListType(a), ListType(e))                 => matches(graph, a, e)
        case (OptionType(a), OptionType(e))             => matches(graph, a, e)
        case (StreamType(Some(a)), StreamType(Some(e))) => matches(graph, a, e)
        case (a, e)                                     => a == e
      }
    }

  private def validateFilesystem(agent: AgentMetadata, mount: HttpMountDetails): Either[String, Unit] = {
    if (agent.mode.exists(_ != "durable") || mount.phantomAgent) return Left("filesystem-owner")
    val captures   = mount.pathPrefix.collect { case PathSegment.PathVariable(name) => name }
    val fields     = agent.constructor.input.parameters
    val validMount = mount.pathPrefix.forall {
      case PathSegment.Literal(s)                                      => FileMappingParser.validSegment(s)
      case PathSegment.PathVariable(_) | PathSegment.SystemVariable(_) => true
      case _                                                           => false
    }
    val validFields = fields.forall { field =>
      field.source == FieldSource.UserSupplied && RefResolution
        .resolveRef(field.graph, field.graph.root)
        .toOption
        .exists(_.body match {
          case StringType | CharType | BoolType | EnumType(_) | U8Type(_) | U16Type(_) | U32Type(_) | U64Type(_) |
              S8Type(_) | S16Type(_) | S32Type(_) | S64Type(_) | F32Type(_) | F64Type(_) =>
            true
          case _ => false
        })
    }
    if (
      !validMount || !validFields || captures.distinct.size != captures.size || captures.sorted != fields
        .map(_.name)
        .sorted
    )
      Left("unbound-constructor")
    else Right(())
  }
}
