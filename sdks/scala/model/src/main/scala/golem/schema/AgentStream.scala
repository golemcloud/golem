/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.schema

import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.collection.mutable
import scala.util.{DynamicVariable, Failure, Success, Try}
import scala.util.control.NonFatal

/**
 * A lazy, pull-based, single-reader stream used by schema-native agent calls.
 * The producer is asked for a value only when [[pull]] is called, and only one
 * pull may be active at a time.
 *
 * An `AgentStream` is affine. Passing it through schema conversion, returning
 * it from an agent method, or calling [[map]] transfers the stream; the
 * original stream can no longer be pulled, closed, or transferred again.
 *
 * For a stream connected through the component protocol, closing the consumer
 * drops its readable endpoint. Producer cancellation is cooperative: the
 * producer observes the drop when a subsequent write is rejected, then closes
 * its source. It cannot interrupt an arbitrary pending source pull, and remote
 * cleanup is not guaranteed to finish before a later invocation. On the
 * producer side, the next source pull starts only after the previous write is
 * accepted, providing backpressure. Acceptance does not guarantee that the
 * value becomes externally observable before an immediately following producer
 * failure.
 *
 * A bare protocol stream has no recoverable terminal error or error reason. At
 * that protocol boundary, producer and cleanup failures fail the active
 * invocation. Applications that need recoverable failures must represent them
 * explicitly in the element type.
 *
 * @tparam A
 *   the type of values produced by this stream
 */
final class AgentStream[+A] private[golem] (
  private[golem] val pullValue: () => Future[Option[A]],
  private[golem] val finalizeValue: () => Future[Unit],
  private var directTransfer: Option[() => GuestSchemaValueStreamHandle] = None,
  private var ownershipEntry: Option[AgentStreamOwnership.Entry] = None
) {
  private sealed trait State
  private case object Open                          extends State
  private case object Pulling                       extends State
  private case object Completed                     extends State
  private final case class Failed(error: Throwable) extends State
  private final case class Closed(error: Throwable) extends State
  private case object Transferred                   extends State

  private val lock                                     = new AnyRef
  private var state: State                             = Open
  private var activePull: Option[Promise[Option[Any]]] = None
  private var finalization: Option[Promise[Unit]]      = None

  /**
   * Requests the next value from the producer.
   *
   * The returned future completes with `Some(value)` for an item and `None`
   * after clean completion. A producer or decoding failure fails the future and
   * is returned by later pulls as well. Calling `pull` while another pull is
   * active returns a failed future.
   */
  def pull(): Future[Option[A]] = {
    val result = lock.synchronized {
      state match {
        case Open =>
          val promise = Promise[Option[Any]]()
          state = Pulling
          activePull = Some(promise)
          directTransfer = None
          val ownership = effectiveOwnership
          val source    =
            try AgentStreamOwnership.capture(ownership)(pullValue())
            catch {
              case NonFatal(error) => Future.failed(error)
            }
          Left((promise, source))
        case Pulling       => Right(Future.failed(new IllegalStateException("agent stream already has an active pull")))
        case Completed     => Right(Future.successful(None))
        case Failed(error) => Right(Future.failed(error))
        case Closed(error) => Right(Future.failed(error))
        case Transferred   => Right(Future.failed(new IllegalStateException("agent stream was already transferred")))
      }
    }

    result match {
      case Right(future)           => future
      case Left((promise, source)) =>
        source.onComplete(completePull(promise, _))(using ExecutionContext.parasitic)
        promise.future.asInstanceOf[Future[Option[A]]]
    }
  }

  /**
   * Stops this stream and releases its producer. Closing is idempotent and
   * waits for the finalizer. An active pull fails immediately; its underlying
   * producer operation may continue, but any later result is ignored.
   *
   * Closing a transferred stream returns a failed future because ownership has
   * moved to another stream or schema value.
   */
  def close(): Future[Unit] = {
    val (pending, result, finalizer) = lock.synchronized {
      state match {
        case Transferred =>
          (None, Future.failed(new IllegalStateException("agent stream was already transferred")), None)
        case Completed | Failed(_) | Closed(_) =>
          val (result, finalizer) = reserveFinalization()
          (None, result, finalizer)
        case Open | Pulling =>
          val error = new IllegalStateException("agent stream was closed")
          val pull  = activePull
          state = Closed(error)
          activePull = None
          directTransfer = None
          val (result, finalizer) = reserveFinalization()
          (pull.map(_ -> error), result, finalizer)
      }
    }
    pending.foreach { case (promise, error) => promise.tryFailure(error) }
    runFinalizer(finalizer)
    result
  }

  /**
   * Lazily transforms each produced value and transfers ownership into the
   * returned stream. The original can no longer be pulled, closed, or
   * transferred.
   */
  def map[B](f: A => B)(implicit ec: ExecutionContext): AgentStream[B] =
    lock.synchronized {
      ensureTransferable()
      val mapped = new AgentStream(
        () => {
          val ownership = effectiveOwnership
          pullValue().map(value => AgentStreamOwnership.capture(ownership)(value.map(f)))
        },
        finalizeValue,
        ownershipEntry = ownershipEntry
      )
      state = Transferred
      directTransfer = None
      ownershipEntry.foreach(_.replace(() => mapped.close()))
      mapped
    }

  private[golem] def moveToSchemaValueStream(
    encode: A => SchemaValue
  )(implicit ec: ExecutionContext): GuestSchemaValueStreamHandle =
    lock.synchronized {
      ensureTransferable()
      val handle = directTransfer match {
        case Some(transfer) => transfer()
        case None           =>
          val nestedOwnership = new AgentStreamOwnership
          val stream          = new AgentStream(
            () => {
              val ownership = effectiveOwnership.orElse(Some(nestedOwnership))
              pullValue().map(value => AgentStreamOwnership.capture(ownership)(value.map(encode)))
            },
            () => {
              val sourceFinalization =
                try finalizeValue()
                catch {
                  case NonFatal(error) => Future.failed(error)
                }
              sourceFinalization.transformWith(completed =>
                nestedOwnership.close().flatMap(_ => Future.fromTry(completed))(using ExecutionContext.parasitic)
              )(using ExecutionContext.parasitic)
            },
            ownershipEntry = ownershipEntry
          )
          GuestSchemaValueStreamHandle.native(stream)
      }
      val endpoint = handle.withHandle(identity).getOrElse {
        throw new IllegalStateException("schema value stream was already transferred")
      }
      state = Transferred
      ownershipEntry.foreach { entry =>
        endpoint.attachOwnership(entry)
        entry.replace(() => endpoint.dispose())
      }
      handle
    }

  private[golem] def ownership: Option[AgentStreamOwnership.Entry] = ownershipEntry

  private def effectiveOwnership: Option[AgentStreamOwnership] =
    ownershipEntry match {
      case Some(entry) => entry.activeOwner
      case None        => AgentStreamOwnership.currentOwner
    }

  private[golem] def attachOwnership(entry: AgentStreamOwnership.Entry): Unit =
    lock.synchronized {
      ownershipEntry match {
        case Some(existing) if existing ne entry =>
          throw new IllegalStateException("agent stream already belongs to another invocation")
        case _ => ownershipEntry = Some(entry)
      }
    }

  private def completePull(promise: Promise[Option[Any]], result: Try[Option[A]]): Unit = {
    val completion = lock.synchronized {
      if (state != Pulling || !activePull.contains(promise)) None
      else {
        activePull = None
        result match {
          case Success(None) =>
            state = Completed
            val (_, finalizer) = reserveFinalization()
            Some((promise, Success(None), finalizer))
          case Success(Some(value)) =>
            state = Open
            Some((promise, Success(Some(value.asInstanceOf[Any])), None))
          case Failure(error) =>
            state = Failed(error)
            val (_, finalizer) = reserveFinalization()
            Some((promise, Failure(error), finalizer))
        }
      }
    }
    completion.foreach { case (target, value, finalizer) =>
      target.tryComplete(value)
      runFinalizer(finalizer)
    }
  }

  private def reserveFinalization(): (Future[Unit], Option[Promise[Unit]]) =
    finalization match {
      case Some(promise) => (promise.future, None)
      case None          =>
        val promise = Promise[Unit]()
        finalization = Some(promise)
        (promise.future, Some(promise))
    }

  private def runFinalizer(finalizer: Option[Promise[Unit]]): Unit =
    finalizer.foreach { promise =>
      val result =
        try finalizeValue()
        catch {
          case NonFatal(error) => Future.failed(error)
        }
      result
        .transformWith(completed =>
          ownershipEntry
            .map(_.closeTransferredOwnership())
            .getOrElse(Future.successful(()))
            .flatMap(_ => Future.fromTry(completed))(using ExecutionContext.parasitic)
        )(using ExecutionContext.parasitic)
        .onComplete(promise.tryComplete)(using ExecutionContext.parasitic)
    }

  private def ensureTransferable(): Unit =
    state match {
      case Open        => ()
      case Pulling     => throw new IllegalStateException("agent stream cannot be transferred while a pull is active")
      case Transferred => throw new IllegalStateException("agent stream was already transferred")
      case Completed   => throw new IllegalStateException("completed agent stream cannot be transferred")
      case Failed(_)   => throw new IllegalStateException("failed agent stream cannot be transferred")
      case Closed(_)   => throw new IllegalStateException("closed agent stream cannot be transferred")
    }
}

object AgentStream {

  /**
   * Creates a lazy stream backed by a pull function.
   *
   * `pull` is invoked only in response to demand and must return `Some(value)`
   * or `None` for clean completion. `onFinalize` is invoked exactly once after
   * clean completion, producer failure, or explicit close;
   * [[AgentStream.close]] waits for it.
   *
   * @param pull
   *   obtains the next value or clean end of stream
   * @param onFinalize
   *   releases resources owned by the producer
   */
  def fromPull[A](
    pull: () => Future[Option[A]],
    onFinalize: () => Future[Unit] = () => Future.successful(())
  ): AgentStream[A] = AgentStreamOwnership.own(new AgentStream(pull, onFinalize))

  implicit def intoSchema[A](implicit element: IntoSchema[A]): IntoSchema[AgentStream[A]] =
    new IntoSchema[AgentStream[A]] {
      private implicit val executionContext: ExecutionContext = ExecutionContext.parasitic

      override lazy val graph: SchemaGraph =
        SchemaGraph(element.graph.defs, SchemaType(SchemaTypeBody.StreamType(Some(element.graph.root))))
      override def toValue(value: AgentStream[A]): SchemaValue =
        SchemaValue.StreamValue(value.moveToSchemaValueStream(element.toValue))
    }

  implicit def fromSchema[A](implicit element: FromSchema[A]): FromSchema[AgentStream[A]] =
    new FromSchema[AgentStream[A]] {
      private implicit val executionContext: ExecutionContext = ExecutionContext.parasitic

      private def decode(
        stream: AgentStream[SchemaValue],
        ownership: Option[AgentStreamOwnership.Entry]
      ): AgentStream[A] = {
        val decoded = new AgentStream(
          () =>
            stream.pull().flatMap {
              case None        => Future.successful(None)
              case Some(value) =>
                AgentStreamOwnership
                  .capture(ownership.flatMap(_.activeOwner)) {
                    element.fromValue(value)
                  }
                  .fold(e => Future.failed(e), a => Future.successful(Some(a)))
            },
          () => stream.close(),
          ownershipEntry = ownership
        )
        ownership.foreach(_.replace(() => decoded.close()))
        AgentStreamOwnership.own(decoded)
      }

      override def fromValue(value: SchemaValue): Either[FromSchemaError, AgentStream[A]] = value match {
        case SchemaValue.StreamValue(handle) =>
          handle.take() match {
            case Some(native: GuestSchemaValueStream.Native) =>
              Right(decode(native.value, native.ownership))
            case Some(wrapped: GuestSchemaValueStream.Wrapped) =>
              lazy val stream = wrapped.unwrap()
              val decoded     =
                new AgentStream(
                  () => stream.flatMap(inner => decode(inner, wrapped.ownership).pull()),
                  () => stream.flatMap(_.close()),
                  Some(() => GuestSchemaValueStreamHandle.endpoint(wrapped)),
                  wrapped.ownership
                )
              wrapped.ownership.foreach(_.replace(() => decoded.close()))
              Right(
                AgentStreamOwnership.own(decoded)
              )
            case None => Left(FromSchemaError("schema value stream was already transferred"))
          }
        case other => Left(FromSchemaError(s"Expected stream value, got $other"))
      }
    }
}

private[golem] sealed trait GuestSchemaValueStream {
  def ownershipKey: Any
  def dispose(): Future[Unit]

  private var ownershipEntry: Option[AgentStreamOwnership.Entry] = None
  private var committed                                          = false

  private[golem] final def ownership: Option[AgentStreamOwnership.Entry] = synchronized(ownershipEntry)

  private[golem] final def activeOwnership: Option[AgentStreamOwnership] =
    ownership.flatMap(_.activeOwner)

  private[golem] final def closeTransferredOwnership(): Future[Unit] =
    ownership.map(_.closeTransferredOwnership()).getOrElse(Future.successful(()))

  private[golem] final def attachOwnership(entry: AgentStreamOwnership.Entry): Unit = synchronized {
    ownershipEntry match {
      case Some(existing) if existing ne entry =>
        throw new IllegalStateException("schema value stream already belongs to another invocation")
      case _ => ownershipEntry = Some(entry)
    }
  }

  private[golem] final def commitTransfer(): Unit = synchronized {
    committed = true
    ownershipEntry.foreach(_.commit())
  }

  private[golem] final def isTransferCommitted: Boolean = synchronized(committed)
}
private[golem] object GuestSchemaValueStream {
  final case class Wrapped(raw: Any, unwrapValue: () => Future[AgentStream[SchemaValue]])
      extends GuestSchemaValueStream {
    private lazy val stream =
      try unwrapValue()
      catch {
        case NonFatal(error) => Future.failed(error)
      }
    private lazy val disposal = stream
      .flatMap(_.close())(using ExecutionContext.parasitic)
      .transformWith(result =>
        AgentStreamOwnership
          .cleanup(closeTransferredOwnership())
          .flatMap(_ => Future.fromTry(result))(using ExecutionContext.parasitic)
      )(using ExecutionContext.parasitic)

    def unwrap(): Future[AgentStream[SchemaValue]] = stream

    override def ownershipKey: Any       = raw
    override def dispose(): Future[Unit] = disposal
  }
  final case class Native(value: AgentStream[SchemaValue]) extends GuestSchemaValueStream {
    value.ownership.foreach(attachOwnership)

    private lazy val disposal = value
      .close()
      .transformWith(result =>
        AgentStreamOwnership
          .cleanup(closeTransferredOwnership())
          .flatMap(_ => Future.fromTry(result))(using ExecutionContext.parasitic)
      )(using ExecutionContext.parasitic)

    override def ownershipKey: Any       = value
    override def dispose(): Future[Unit] = disposal
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
  ): GuestSchemaValueStreamHandle = endpoint(GuestSchemaValueStream.Wrapped(raw, unwrap))

  private[golem] def endpoint(value: GuestSchemaValueStream): GuestSchemaValueStreamHandle = {
    AgentStreamOwnership.own(value)
    AgentStreamOutputTransaction.track(value)
    new GuestSchemaValueStreamHandle(Some(value))
  }

  private[golem] def native(stream: AgentStream[SchemaValue]): GuestSchemaValueStreamHandle =
    endpoint(GuestSchemaValueStream.Native(stream))
}

private[golem] final class AgentStreamOwnership {
  private implicit val executionContext: ExecutionContext = ExecutionContext.parasitic
  private val entries                                     = mutable.ListBuffer.empty[AgentStreamOwnership.Entry]
  private var closed                                      = false

  private[golem] def register(cleanup: () => Future[Unit]): AgentStreamOwnership.Entry = synchronized {
    val entry = new AgentStreamOwnership.Entry(this, cleanup)
    if (!closed) entries += entry
    else AgentStreamOwnership.cleanup(entry.close())
    entry
  }

  def close(): Future[Unit] = {
    val owned = synchronized {
      if (closed) Nil
      else {
        closed = true
        entries.toList
      }
    }
    Future
      .sequence(owned.map(entry => AgentStreamOwnership.cleanup(entry.close())))
      .map(_ => ())
  }
}

private[golem] object AgentStreamOwnership {
  private val current = new DynamicVariable[Option[AgentStreamOwnership]](None)

  private[golem] def currentOwner: Option[AgentStreamOwnership] = current.value

  final class Entry private[AgentStreamOwnership] (
    val owner: AgentStreamOwnership,
    initialCleanup: () => Future[Unit]
  ) {
    private var cleanup                                            = initialCleanup
    private var state: Either[Boolean, Promise[Unit]]              = Left(false)
    private var transferredOwnership: Option[AgentStreamOwnership] = None

    def replace(action: () => Future[Unit]): Unit = synchronized {
      state match {
        case Left(false) => cleanup = action
        case _           => ()
      }
    }

    def commit(): Unit = synchronized {
      state match {
        case Left(false) =>
          transferredOwnership = Some(new AgentStreamOwnership)
          state = Left(true)
        case _ => ()
      }
    }

    def activeOwner: Option[AgentStreamOwnership] = synchronized {
      state match {
        case Left(false) => Some(owner)
        case Left(true)  => transferredOwnership
        case Right(_)    => None
      }
    }

    def closeTransferredOwnership(): Future[Unit] = {
      val transferred = synchronized {
        val result = transferredOwnership
        transferredOwnership = None
        result
      }
      transferred.map(_.close()).getOrElse(Future.successful(()))
    }

    def close(): Future[Unit] = {
      val result = synchronized {
        state match {
          case Left(true)  => Right(Future.successful(()))
          case Right(done) => Right(done.future)
          case Left(false) =>
            val done = Promise[Unit]()
            state = Right(done)
            Left((done, cleanup))
        }
      }
      result match {
        case Right(done)          => done
        case Left((done, action)) =>
          val closing =
            try action()
            catch {
              case NonFatal(error) => Future.failed(error)
            }
          closing.onComplete(done.tryComplete)(using ExecutionContext.parasitic)
          done.future
      }
    }
  }

  def capture[A](ownership: AgentStreamOwnership)(body: => A): A =
    current.withValue(Some(ownership))(body)

  def capture[A](ownership: Option[AgentStreamOwnership])(body: => A): A =
    current.withValue(ownership)(body)

  def own[A](stream: AgentStream[A]): AgentStream[A] = {
    if (stream.ownership.isEmpty) {
      current.value.foreach { owner =>
        val entry = owner.register(() => stream.close())
        stream.attachOwnership(entry)
      }
    }
    stream
  }

  def own(stream: GuestSchemaValueStream): GuestSchemaValueStream = {
    if (stream.ownership.isEmpty) {
      current.value.foreach { owner =>
        val entry = owner.register(() => stream.dispose())
        stream.attachOwnership(entry)
      }
    }
    stream
  }

  def cleanup(action: => Future[Unit]): Future[Unit] =
    try action.recover { case _ => () }(using ExecutionContext.parasitic)
    catch {
      case _: Throwable => Future.successful(())
    }
}

private[golem] final class AgentStreamOutputTransaction {
  private implicit val executionContext: ExecutionContext = ExecutionContext.parasitic
  private val streams                                     = mutable.ListBuffer.empty[GuestSchemaValueStream]

  def register(stream: GuestSchemaValueStream): Unit = synchronized {
    if (!streams.exists(_ eq stream)) streams += stream
  }

  def rollback(): Future[Unit] =
    close(streams.toList.reverse)

  def closeUncommitted(): Future[Unit] =
    close(streams.toList.reverse.filterNot(_.isTransferCommitted))

  private def close(values: List[GuestSchemaValueStream]): Future[Unit] =
    Future.sequence(values.map(stream => AgentStreamOwnership.cleanup(stream.dispose()))).map(_ => ())
}

private[golem] object AgentStreamOutputTransaction {
  private val current = new DynamicVariable[Option[AgentStreamOutputTransaction]](None)

  def capture[A](transaction: AgentStreamOutputTransaction)(body: => A): A =
    current.withValue(Some(transaction))(body)

  def track(stream: GuestSchemaValueStream): Unit =
    current.value.foreach(_.register(stream))
}
