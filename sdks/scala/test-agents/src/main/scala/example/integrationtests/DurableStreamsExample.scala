/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package example.integrationtests

import golem.BaseAgent
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import golem.schema.AgentStream
import golem.streams.*
import scala.concurrent.{ExecutionContext, Future}

@agentDefinition()
trait DurableStreamsExample extends BaseAgent {
  class Id(val url: String, val producerId: String)

  def readJson(offset: String): Future[golem.schema.AgentStream[Long]]
  def readBytes(offset: String): Future[golem.schema.AgentStream[Byte]]
  def nested(offset: String): Future[golem.schema.AgentStream[golem.schema.AgentStream[Long]]]
  def consume(input: golem.schema.AgentStream[Long]): Future[Vector[Long]]
  def forward(targetUrl: String, targetProducer: String, offset: String): Future[Vector[Long]]
  def appendJson(values: Vector[Long], close: Boolean): Future[Option[String]]
  def appendBytes(values: Vector[Byte], close: Boolean): Future[Option[String]]
  def retryJson(): Future[Option[String]]
}

@agentImplementation()
final class DurableStreamsExampleImpl(url: String, producerId: String) extends DurableStreamsExample {
  private implicit val ec: ExecutionContext = ExecutionContext.parasitic
  private lazy val jsonWriter               = DurableStreams.jsonWriter[Long](url, DurableStreamProducer(producerId))
  private lazy val byteWriter               = DurableStreams.byteWriter(url, producer = DurableStreamProducer(producerId))

  override def readJson(offset: String): Future[AgentStream[Long]] =
    Future.successful(DurableStreams.json[Long](url, DurableStreamReadOptions(DurableStreamCheckpoint(offset))))

  override def readBytes(offset: String): Future[AgentStream[Byte]] =
    Future.successful(DurableStreams.bytes(url, DurableStreamReadOptions(DurableStreamCheckpoint(offset))))

  override def nested(offset: String): Future[AgentStream[AgentStream[Long]]] = readJson(offset).map { inner =>
    var emitted = false
    AgentStream.fromPull { () =>
      val value = if (emitted) None else Some(inner)
      emitted = true
      Future.successful(value)
    }
  }

  override def consume(input: AgentStream[Long]): Future[Vector[Long]] = {
    def loop(values: Vector[Long]): Future[Vector[Long]] = input.pull().flatMap {
      case Some(value) => loop(values :+ value)
      case None        => Future.successful(values)
    }
    loop(Vector.empty)
  }

  override def forward(targetUrl: String, targetProducer: String, offset: String): Future[Vector[Long]] =
    readJson(offset).flatMap(stream => DurableStreamsExampleClient.get(targetUrl, targetProducer).consume(stream))

  override def appendJson(values: Vector[Long], close: Boolean): Future[Option[String]] =
    jsonWriter.append(values, close).map(_.nextOffset)

  override def appendBytes(values: Vector[Byte], close: Boolean): Future[Option[String]] =
    byteWriter.append(values, close).map(_.nextOffset)

  override def retryJson(): Future[Option[String]] = jsonWriter.retryPending().map(_.nextOffset)
}
