package faultmatrix

// The tool of the fault-matrix fixture contract: `produce` writes the records of `produce` to
// stdout, each as 4 big-endian bytes, and awaits `gate` before record `pauseAfter + 1`.

import golem.{HostApi, UInt}
import golem.runtime.annotations.{toolDefinition, toolImplementation}
import golem.tool.ToolOutputStream

import java.nio.ByteBuffer
import scala.concurrent.{ExecutionContext, Future}

@toolDefinition(name = "fault-matrix", version = "1.0.0")
trait FaultMatrixTool {
  def produce(run: UInt, count: UInt, pauseAfter: UInt, gate: Gate, stdout: ToolOutputStream): Future[UInt]
}

@toolImplementation()
final class FaultMatrixToolImpl extends FaultMatrixTool {
  private implicit val ec: ExecutionContext = ExecutionContext.parasitic

  private def recordBytes(run: UInt, seq: Long): Array[Byte] =
    ByteBuffer.allocate(4).putInt((run.value * 100000L + seq).toInt).array()

  override def produce(
      run: UInt,
      count: UInt,
      pauseAfter: UInt,
      gate: Gate,
      stdout: ToolOutputStream
  ): Future[UInt] = {
    def write(seq: Long): Future[UInt] =
      if (seq > count.value) stdout.finish().map(_ => count)
      else {
        val gated =
          if (seq == pauseAfter.value + 1) HostApi.awaitPromise(gate.toHost).map(_ => ())
          else Future.unit
        gated.flatMap(_ => stdout.write(recordBytes(run, seq))).flatMap {
          case Right(_) => write(seq + 1)
          case Left(_)  => Future.successful(UInt(seq - 1))
        }
      }
    write(1)
  }
}
