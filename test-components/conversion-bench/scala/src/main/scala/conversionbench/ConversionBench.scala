package conversionbench

import golem.{BaseAgent, UByte, UInt}
import golem.runtime.annotations.*

@agentDefinition()
trait ConversionBenchScala extends BaseAgent {
  class Id(val name: String)
  def checksum(input: List[golem.UByte]): golem.UInt
  def produce(length: golem.UInt): List[golem.UByte]
}

@agentImplementation()
final class ConversionBenchScalaImpl(name: String) extends ConversionBenchScala {
  override def checksum(input: List[UByte]): UInt =
    UInt(input.foldLeft(0L)((sum, byte) => (sum + byte.value) & 0xffffffffL))

  override def produce(length: UInt): List[UByte] =
    List.tabulate(length.value.toInt)(i => UByte((i % 251).toShort))
}
