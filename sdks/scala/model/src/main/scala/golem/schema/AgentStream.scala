/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.schema

import scala.concurrent.{ExecutionContext, Future}
import scala.util.control.NonFatal

/** A demand-driven, single-reader stream used by schema-native agent calls. */
final class AgentStream[+A] private[golem] (
  private[golem] val pullValue: () => Future[Option[A]],
  private var directTransfer: Option[() => GuestSchemaValueStreamHandle] = None
) {
  private val lock       = new AnyRef
  private var moved      = false
  private var pullActive = false

  def pull(): Future[Option[A]] = lock.synchronized {
    if (moved)
      Future.failed(new IllegalStateException("agent stream was already transferred"))
    else if (pullActive)
      Future.failed(new IllegalStateException("agent stream already has an active pull"))
    else {
      pullActive = true
      directTransfer = None
      val result =
        try pullValue()
        catch {
          case NonFatal(error) => Future.failed(error)
        }
      result.andThen { case _ => lock.synchronized { pullActive = false } }(ExecutionContext.parasitic)
    }
  }

  /**
   * Transfers this stream into a mapped stream. The original can no longer be
   * pulled or transferred.
   */
  def map[B](f: A => B)(implicit ec: ExecutionContext): AgentStream[B] =
    lock.synchronized {
      if (moved)
        throw new IllegalStateException("agent stream was already transferred")
      if (pullActive)
        throw new IllegalStateException("agent stream cannot be transferred while a pull is active")
      moved = true
      directTransfer = None
      AgentStream.fromPull(() => pullValue().map(_.map(f)))
    }

  private[golem] def moveToSchemaValueStream(
    encode: A => SchemaValue
  )(implicit ec: ExecutionContext): GuestSchemaValueStreamHandle =
    lock.synchronized {
      if (moved)
        throw new IllegalStateException("agent stream was already transferred")
      if (pullActive)
        throw new IllegalStateException("agent stream cannot be transferred while a pull is active")
      moved = true
      directTransfer match {
        case Some(transfer) => transfer()
        case None           => GuestSchemaValueStreamHandle.native(AgentStream.fromPull(() => pullValue().map(_.map(encode))))
      }
    }
}

object AgentStream {
  def fromPull[A](pull: () => Future[Option[A]]): AgentStream[A] = new AgentStream(pull)

  implicit def intoSchema[A](implicit element: IntoSchema[A], ec: ExecutionContext): IntoSchema[AgentStream[A]] =
    new IntoSchema[AgentStream[A]] {
      override lazy val graph: SchemaGraph =
        SchemaGraph(element.graph.defs, SchemaType(SchemaTypeBody.StreamType(Some(element.graph.root))))
      override def toValue(value: AgentStream[A]): SchemaValue =
        SchemaValue.StreamValue(value.moveToSchemaValueStream(element.toValue))
    }

  implicit def fromSchema[A](implicit element: FromSchema[A], ec: ExecutionContext): FromSchema[AgentStream[A]] =
    new FromSchema[AgentStream[A]] {
      private def decode(stream: AgentStream[SchemaValue]): AgentStream[A] =
        new AgentStream(() =>
          stream.pull().flatMap {
            case None        => Future.successful(None)
            case Some(value) =>
              element.fromValue(value).fold(e => Future.failed(e), a => Future.successful(Some(a)))
          }
        )

      override def fromValue(value: SchemaValue): Either[FromSchemaError, AgentStream[A]] = value match {
        case SchemaValue.StreamValue(handle) =>
          handle.take() match {
            case Some(GuestSchemaValueStream.Native(stream)) =>
              Right(decode(stream))
            case Some(wrapped @ GuestSchemaValueStream.Wrapped(_, unwrap)) =>
              lazy val stream = unwrap()
              Right(
                new AgentStream(
                  () => stream.flatMap(decode(_).pull()),
                  Some(() => GuestSchemaValueStreamHandle.endpoint(wrapped))
                )
              )
            case None => Left(FromSchemaError("schema value stream was already transferred"))
          }
        case other => Left(FromSchemaError(s"Expected stream value, got $other"))
      }
    }
}

private[golem] sealed trait GuestSchemaValueStream {
  def ownershipKey: Any
}
private[golem] object GuestSchemaValueStream {
  final case class Wrapped(raw: Any, unwrap: () => Future[AgentStream[SchemaValue]]) extends GuestSchemaValueStream {
    override def ownershipKey: Any = raw
  }
  final case class Native(value: AgentStream[SchemaValue]) extends GuestSchemaValueStream {
    override def ownershipKey: Any = value
  }
}

/**
 * Affine holder for a schema value stream travelling in a schema value tree.
 */
final class GuestSchemaValueStreamHandle private (private var cell: Option[GuestSchemaValueStream]) {
  def isPresent: Boolean                                                      = cell.isDefined
  private[golem] def take(): Option[GuestSchemaValueStream]                   = { val result = cell; cell = None; result }
  private[golem] def withHandle[A](f: GuestSchemaValueStream => A): Option[A] = cell.map(f)
  private[golem] def ownershipKey: Option[Any]                                = cell.map(_.ownershipKey)
}

object GuestSchemaValueStreamHandle {
  private[golem] def wrapped(
    raw: Any,
    unwrap: () => Future[AgentStream[SchemaValue]]
  ): GuestSchemaValueStreamHandle =
    endpoint(GuestSchemaValueStream.Wrapped(raw, unwrap))

  private[golem] def endpoint(value: GuestSchemaValueStream): GuestSchemaValueStreamHandle =
    new GuestSchemaValueStreamHandle(Some(value))

  private[golem] def native(stream: AgentStream[SchemaValue]): GuestSchemaValueStreamHandle =
    new GuestSchemaValueStreamHandle(Some(GuestSchemaValueStream.Native(stream)))
}
