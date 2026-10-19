package example.integrationtests

import golem.BaseAgent
import golem.runtime.*
import golem.runtime.annotations.*
import zio.blocks.schema.Schema
import scala.concurrent.Future

final case class Saved(revision: Int) derives Schema

@agentDefinition("MultipartIndex", snapshotting = "every(10)")
trait MultipartIndex extends BaseAgent {
  class Id(val name: String)
  def append(byte: Int): Unit
  def inspect(): String
}

@agentImplementation()
final class MultipartIndexImpl(name: String) extends MultipartIndex {
  private var revision = 0
  private var index = (0 to 255).map(_.toByte).toArray
  private var restored = false
  def append(byte: Int): Unit = { revision += 1; index = index :+ byte.toByte }
  def inspect(): String = s"revision-0|$revision|$restored|${index.map(_ & 255).mkString(",")}"
  def saveSnapshotParts(): Future[MultipartSnapshot] = Future.fromTry(
    MultipartSnapshot.fromState(Saved(revision), Map("index" -> SnapshotPart(index.clone(), "application/octet-stream"))).toTry
  )
}
object MultipartIndexImpl {
  def loadSnapshotParts(snapshot: MultipartSnapshot, context: SnapshotRestoreContext): Future[MultipartIndexImpl] =
    Future.fromTry((for {
      saved <- snapshot.decodeState[Saved]
      bytes <- snapshot.requirePart("index", "application/octet-stream")
    } yield {
      val result = new MultipartIndexImpl(context.identity[String](0))
      result.revision = saved.revision
      result.index = bytes.clone()
      result.restored = true
      result
    }).toTry)
}
