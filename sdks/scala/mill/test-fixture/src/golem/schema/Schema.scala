package golem.schema

trait SchemaValue
final case class TypedSchemaValue()

trait IntoSchema[-A] {
  def toValue(value: A): SchemaValue
  def graph: Any
}

object IntoSchema {
  given [A]: IntoSchema[A] with {
    def toValue(value: A): SchemaValue = new SchemaValue {}
    def graph: Any                         = ()
  }
}

trait FromSchema[+A]

object FromSchema {
  given [A]: FromSchema[A] with {}
}
