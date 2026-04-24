/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.host

import golem.host.js._

import java.util.concurrent.TimeUnit
import scala.collection.mutable
import scala.concurrent.duration.FiniteDuration
import scala.scalajs.js
import scala.scalajs.js.JSConverters._

/**
 * Scala-native retry policy builders and validated conversion to raw host
 * facades.
 */
object Retry {
  final case class ValidationError(message: String)

  private sealed trait DecodeError extends Throwable {
    def message: String

    final override def getMessage: String = message
  }

  private final case class InvalidRetryShape(message: String) extends RuntimeException(message) with DecodeError

  sealed trait PredicateValue

  object PredicateValue {
    final case class Text(value: String)                extends PredicateValue
    final case class Integer(value: BigInt)             extends PredicateValue
    final case class BooleanValue(value: scala.Boolean) extends PredicateValue

    trait Encoder[-A] {
      def encode(value: A): PredicateValue
    }

    object Encoder {
      implicit val predicateValueEncoder: Encoder[PredicateValue] = new Encoder[PredicateValue] {
        override def encode(value: PredicateValue): PredicateValue = value
      }

      implicit val stringEncoder: Encoder[String] = new Encoder[String] {
        override def encode(value: String): PredicateValue = Text(value)
      }

      implicit val intEncoder: Encoder[Int] = new Encoder[Int] {
        override def encode(value: Int): PredicateValue = Integer(BigInt(value))
      }

      implicit val longEncoder: Encoder[Long] = new Encoder[Long] {
        override def encode(value: Long): PredicateValue = Integer(BigInt(value))
      }

      implicit val bigIntEncoder: Encoder[BigInt] = new Encoder[BigInt] {
        override def encode(value: BigInt): PredicateValue = Integer(value)
      }

      implicit val booleanEncoder: Encoder[scala.Boolean] = new Encoder[scala.Boolean] {
        override def encode(value: scala.Boolean): PredicateValue = BooleanValue(value)
      }
    }

    def text(value: String): PredicateValue = Text(value)

    def integer(value: BigInt): PredicateValue = Integer(value)

    def integer(value: Long): PredicateValue = Integer(BigInt(value))

    def boolean(value: scala.Boolean): PredicateValue = BooleanValue(value)

    private[golem] def toJs(value: PredicateValue, path: String): Either[ValidationError, JsPredicateValue] =
      value match {
        case Text(text) =>
          Right(JsPredicateValue.text(text))
        case Integer(integer) =>
          if (integer < MinS64 || integer > MaxS64) {
            Left(ValidationError(s"$path must fit a signed 64-bit integer, got $integer"))
          } else {
            Right(JsPredicateValue.integer(js.BigInt(integer.toString)))
          }
        case BooleanValue(boolean) =>
          Right(JsPredicateValue.boolean(boolean))
      }

    private[golem] def fromJs(raw: JsPredicateValue): PredicateValue =
      raw.tag match {
        case "text"    => Text(raw.asInstanceOf[JsPredicateValueText].value)
        case "integer" =>
          val value = BigInt(raw.asInstanceOf[JsPredicateValueInteger].value.toString)
          if (value < MinS64 || value > MaxS64) {
            throw InvalidRetryShape(s"Predicate integer value must fit a signed 64-bit integer, got $value")
          }
          Integer(value)
        case "boolean" => BooleanValue(raw.asInstanceOf[JsPredicateValueBoolean].value)
        case other     => throw new IllegalArgumentException(s"Unsupported retry predicate value tag: $other")
      }
  }

  final case class Property(name: String, value: PredicateValue)

  final class Prop private[golem] (val name: String) extends AnyVal {
    import PredicateValue.Encoder

    def ->[A](value: A)(implicit encoder: Encoder[A]): Property =
      Property(name, encoder.encode(value))

    def eq[A](value: A)(implicit encoder: Encoder[A]): Predicate =
      Predicate.eq(name, value)

    def neq[A](value: A)(implicit encoder: Encoder[A]): Predicate =
      Predicate.neq(name, value)

    def gt[A](value: A)(implicit encoder: Encoder[A]): Predicate =
      Predicate.gt(name, value)

    def gte[A](value: A)(implicit encoder: Encoder[A]): Predicate =
      Predicate.gte(name, value)

    def lt[A](value: A)(implicit encoder: Encoder[A]): Predicate =
      Predicate.lt(name, value)

    def lte[A](value: A)(implicit encoder: Encoder[A]): Predicate =
      Predicate.lte(name, value)

    def oneOf[A](values: A*)(implicit encoder: Encoder[A]): Predicate =
      Predicate.OneOf(name, values.toList.map(encoder.encode))

    def matchesGlob(pattern: String): Predicate =
      Predicate.matchesGlob(name, pattern)

    def startsWith(prefix: String): Predicate =
      Predicate.startsWith(name, prefix)

    def contains(substring: String): Predicate =
      Predicate.contains(name, substring)

    def exists: Predicate =
      Predicate.exists(name)
  }

  object Prop {
    private[golem] def apply(name: String): Prop = new Prop(name)
  }

  object Props {
    val verb: Prop              = Prop("verb")
    val nounUri: Prop           = Prop("noun-uri")
    val uriScheme: Prop         = Prop("uri-scheme")
    val uriHost: Prop           = Prop("uri-host")
    val uriPort: Prop           = Prop("uri-port")
    val uriPath: Prop           = Prop("uri-path")
    val statusCode: Prop        = Prop("status-code")
    val errorType: Prop         = Prop("error-type")
    val function: Prop          = Prop("function")
    val targetComponentId: Prop = Prop("target-component-id")
    val targetAgentType: Prop   = Prop("target-agent-type")
    val dbType: Prop            = Prop("db-type")
    val trapType: Prop          = Prop("trap-type")

    def custom(name: String): Prop = Prop(name)

    def apply(name: String): Prop = custom(name)
  }

  sealed trait Predicate { self =>
    def and(that: Predicate): Predicate = Predicate.And(self, that)

    def or(that: Predicate): Predicate = Predicate.Or(self, that)

    def not: Predicate = Predicate.Not(self)
  }

  object Predicate {
    final case class Eq(property: String, value: PredicateValue)           extends Predicate
    final case class Neq(property: String, value: PredicateValue)          extends Predicate
    final case class Gt(property: String, value: PredicateValue)           extends Predicate
    final case class Gte(property: String, value: PredicateValue)          extends Predicate
    final case class Lt(property: String, value: PredicateValue)           extends Predicate
    final case class Lte(property: String, value: PredicateValue)          extends Predicate
    final case class Exists(property: String)                              extends Predicate
    final case class OneOf(property: String, values: List[PredicateValue]) extends Predicate
    final case class MatchesGlob(property: String, pattern: String)        extends Predicate
    final case class StartsWith(property: String, prefix: String)          extends Predicate
    final case class Contains(property: String, substring: String)         extends Predicate
    final case class And(left: Predicate, right: Predicate)                extends Predicate
    final case class Or(left: Predicate, right: Predicate)                 extends Predicate
    final case class Not(inner: Predicate)                                 extends Predicate
    case object Always                                                     extends Predicate
    case object Never                                                      extends Predicate

    import PredicateValue.Encoder

    def always: Predicate = Always

    def never: Predicate = Never

    def eq[A](property: String, value: A)(implicit encoder: Encoder[A]): Predicate =
      Eq(property, encoder.encode(value))

    def neq[A](property: String, value: A)(implicit encoder: Encoder[A]): Predicate =
      Neq(property, encoder.encode(value))

    def gt[A](property: String, value: A)(implicit encoder: Encoder[A]): Predicate =
      Gt(property, encoder.encode(value))

    def gte[A](property: String, value: A)(implicit encoder: Encoder[A]): Predicate =
      Gte(property, encoder.encode(value))

    def lt[A](property: String, value: A)(implicit encoder: Encoder[A]): Predicate =
      Lt(property, encoder.encode(value))

    def lte[A](property: String, value: A)(implicit encoder: Encoder[A]): Predicate =
      Lte(property, encoder.encode(value))

    def exists(property: String): Predicate =
      Exists(property)

    def oneOf[A](property: String, values: A*)(implicit encoder: Encoder[A]): Predicate =
      OneOf(property, values.toList.map(encoder.encode))

    def matchesGlob(property: String, pattern: String): Predicate =
      MatchesGlob(property, pattern)

    def startsWith(property: String, prefix: String): Predicate =
      StartsWith(property, prefix)

    def contains(property: String, substring: String): Predicate =
      Contains(property, substring)

    def toJs(predicate: Predicate): Either[ValidationError, JsRetryPredicate] = {
      val nodes = js.Array[JsPredicateNode]()

      def append(current: Predicate, path: String): Either[ValidationError, Int] = {
        val index = nodes.length
        nodes.push(null.asInstanceOf[JsPredicateNode])

        val built = current match {
          case Eq(property, value) =>
            comparisonNode(property, value, path, JsPredicateNode.propEq)
          case Neq(property, value) =>
            comparisonNode(property, value, path, JsPredicateNode.propNeq)
          case Gt(property, value) =>
            comparisonNode(property, value, path, JsPredicateNode.propGt)
          case Gte(property, value) =>
            comparisonNode(property, value, path, JsPredicateNode.propGte)
          case Lt(property, value) =>
            comparisonNode(property, value, path, JsPredicateNode.propLt)
          case Lte(property, value) =>
            comparisonNode(property, value, path, JsPredicateNode.propLte)
          case Exists(property) =>
            Right(JsPredicateNode.propExists(property))
          case OneOf(property, values) =>
            sequence(values.zipWithIndex.map { case (value, valueIndex) =>
              PredicateValue.toJs(value, s"$path.values[$valueIndex]")
            }).map { rawValues =>
              JsPredicateNode.propIn(JsPropertySetCheck(property, rawValues.toJSArray))
            }
          case MatchesGlob(property, pattern) =>
            Right(JsPredicateNode.propMatches(JsPropertyPattern(property, pattern)))
          case StartsWith(property, prefix) =>
            Right(JsPredicateNode.propStartsWith(JsPropertyPattern(property, prefix)))
          case Contains(property, substring) =>
            Right(JsPredicateNode.propContains(JsPropertyPattern(property, substring)))
          case And(left, right) =>
            for {
              leftIndex  <- append(left, s"$path.left")
              rightIndex <- append(right, s"$path.right")
            } yield JsPredicateNode.predAnd(leftIndex, rightIndex)
          case Or(left, right) =>
            for {
              leftIndex  <- append(left, s"$path.left")
              rightIndex <- append(right, s"$path.right")
            } yield JsPredicateNode.predOr(leftIndex, rightIndex)
          case Not(inner) =>
            append(inner, s"$path.inner").map(JsPredicateNode.predNot)
          case Always =>
            Right(JsPredicateNode.predTrue)
          case Never =>
            Right(JsPredicateNode.predFalse)
        }

        built.map { node =>
          nodes.update(index, node)
          index
        }
      }

      append(predicate, "predicate").map(_ => JsRetryPredicate(nodes))
    }

    def fromJs(raw: JsRetryPredicate): Predicate = {
      val nodes = raw.nodes
      if (nodes.isEmpty) throw new IllegalArgumentException("Retry predicate must contain at least one node")

      val cache      = mutable.Map.empty[Int, Predicate]
      val inProgress = mutable.Set.empty[Int]

      def requireValidIndex(index: Int, path: String): Unit =
        if (index < 0 || index >= nodes.length) {
          throw InvalidRetryShape(s"$path references predicate node $index, but only ${nodes.length} nodes exist")
        }

      def build(index: Int, path: String): Predicate = {
        requireValidIndex(index, path)
        cache.getOrElseUpdate(
          index, {
            if (!inProgress.add(index)) {
              throw InvalidRetryShape(s"$path contains a cycle at predicate node $index")
            }

            try {
              val node = nodes(index)
              if (node == null) {
                throw InvalidRetryShape(s"$path points to a null predicate node")
              }

              node.tag match {
                case "prop-eq" =>
                  val value = node.asInstanceOf[JsPredicateNodeComparison].value
                  Eq(value.propertyName, PredicateValue.fromJs(value.value))
                case "prop-neq" =>
                  val value = node.asInstanceOf[JsPredicateNodeComparison].value
                  Neq(value.propertyName, PredicateValue.fromJs(value.value))
                case "prop-gt" =>
                  val value = node.asInstanceOf[JsPredicateNodeComparison].value
                  Gt(value.propertyName, PredicateValue.fromJs(value.value))
                case "prop-gte" =>
                  val value = node.asInstanceOf[JsPredicateNodeComparison].value
                  Gte(value.propertyName, PredicateValue.fromJs(value.value))
                case "prop-lt" =>
                  val value = node.asInstanceOf[JsPredicateNodeComparison].value
                  Lt(value.propertyName, PredicateValue.fromJs(value.value))
                case "prop-lte" =>
                  val value = node.asInstanceOf[JsPredicateNodeComparison].value
                  Lte(value.propertyName, PredicateValue.fromJs(value.value))
                case "prop-exists" =>
                  Exists(node.asInstanceOf[JsPredicateNodeStringValue].value)
                case "prop-in" =>
                  val value = node.asInstanceOf[JsPredicateNodeSetCheck].value
                  OneOf(value.propertyName, value.values.toList.map(PredicateValue.fromJs))
                case "prop-matches" =>
                  val value = node.asInstanceOf[JsPredicateNodePattern].value
                  MatchesGlob(value.propertyName, value.pattern)
                case "prop-starts-with" =>
                  val value = node.asInstanceOf[JsPredicateNodePattern].value
                  StartsWith(value.propertyName, value.pattern)
                case "prop-contains" =>
                  val value = node.asInstanceOf[JsPredicateNodePattern].value
                  Contains(value.propertyName, value.pattern)
                case "pred-and" =>
                  val value = node.asInstanceOf[JsPredicateNodePair].value
                  And(build(value._1, s"$path.left"), build(value._2, s"$path.right"))
                case "pred-or" =>
                  val value = node.asInstanceOf[JsPredicateNodePair].value
                  Or(build(value._1, s"$path.left"), build(value._2, s"$path.right"))
                case "pred-not" =>
                  Not(build(node.asInstanceOf[JsPredicateNodeIndex].value, s"$path.inner"))
                case "pred-true" =>
                  Always
                case "pred-false" =>
                  Never
                case other =>
                  throw new IllegalArgumentException(s"Unsupported retry predicate node tag: $other")
              }
            } catch {
              case error: DecodeError => throw error
              case error: Throwable   =>
                throw InvalidRetryShape(
                  s"$path contains malformed predicate payload: ${Option(error.getMessage).getOrElse(error.getClass.getSimpleName)}"
                )
            } finally {
              inProgress.remove(index)
            }
          }
        )
      }

      try {
        build(0, "predicate.root")
      } catch {
        case error: DecodeError => throw new IllegalArgumentException(error.message, error)
      }
    }

    private def comparisonNode(
      property: String,
      value: PredicateValue,
      path: String,
      build: JsPropertyComparison => JsPredicateNode
    ): Either[ValidationError, JsPredicateNode] =
      PredicateValue.toJs(value, s"$path.value").map { raw =>
        build(JsPropertyComparison(property, raw))
      }
  }

  sealed trait Policy { self =>
    def maxRetries(maxRetries: Long): Policy = Policy.CountBox(maxRetries, self)

    def within(limit: FiniteDuration): Policy = Policy.TimeBox(limit, self)

    def clamp(minDelay: FiniteDuration, maxDelay: FiniteDuration): Policy =
      Policy.Clamp(minDelay, maxDelay, self)

    def addDelay(delay: FiniteDuration): Policy = Policy.AddDelay(delay, self)

    def withJitter(factor: Double): Policy = Policy.Jitter(factor, self)

    def onlyWhen(predicate: Predicate): Policy = Policy.FilteredOn(predicate, self)

    def andThen(that: Policy): Policy = Policy.AndThen(self, that)

    def union(that: Policy): Policy = Policy.Union(self, that)

    def intersect(that: Policy): Policy = Policy.Intersect(self, that)
  }

  object Policy {
    final case class Periodic(delay: FiniteDuration)                                          extends Policy
    final case class Exponential(baseDelay: FiniteDuration, factor: Double)                   extends Policy
    final case class Fibonacci(first: FiniteDuration, second: FiniteDuration)                 extends Policy
    case object Immediate                                                                     extends Policy
    case object Never                                                                         extends Policy
    final case class CountBox(maxRetries: Long, inner: Policy)                                extends Policy
    final case class TimeBox(limit: FiniteDuration, inner: Policy)                            extends Policy
    final case class Clamp(minDelay: FiniteDuration, maxDelay: FiniteDuration, inner: Policy) extends Policy
    final case class AddDelay(delay: FiniteDuration, inner: Policy)                           extends Policy
    final case class Jitter(factor: Double, inner: Policy)                                    extends Policy
    final case class FilteredOn(predicate: Predicate, inner: Policy)                          extends Policy
    final case class AndThen(left: Policy, right: Policy)                                     extends Policy
    final case class Union(left: Policy, right: Policy)                                       extends Policy
    final case class Intersect(left: Policy, right: Policy)                                   extends Policy

    def immediate: Policy = Immediate

    def never: Policy = Never

    def periodic(delay: FiniteDuration): Policy = Periodic(delay)

    def exponential(baseDelay: FiniteDuration, factor: Double): Policy = Exponential(baseDelay, factor)

    def fibonacci(first: FiniteDuration, second: FiniteDuration): Policy = Fibonacci(first, second)

    def toJs(policy: Policy): Either[ValidationError, JsRetryPolicyTree] = {
      val nodes = js.Array[JsPolicyNode]()

      def append(current: Policy, path: String): Either[ValidationError, Int] = {
        val index = nodes.length
        nodes.push(null.asInstanceOf[JsPolicyNode])

        val built = current match {
          case Periodic(delay) =>
            durationToJs(delay, s"$path.delay").map(JsPolicyNode.periodic)
          case Exponential(baseDelay, factor) =>
            for {
              rawBaseDelay <- durationToJs(baseDelay, s"$path.baseDelay")
              _            <- validatePositiveFiniteDouble(factor, s"$path.factor", allowZero = false)
            } yield JsPolicyNode.exponential(JsExponentialConfig(rawBaseDelay, factor))
          case Fibonacci(first, second) =>
            for {
              rawFirst  <- durationToJs(first, s"$path.first")
              rawSecond <- durationToJs(second, s"$path.second")
            } yield JsPolicyNode.fibonacci(JsFibonacciConfig(rawFirst, rawSecond))
          case Immediate =>
            Right(JsPolicyNode.immediate)
          case Never =>
            Right(JsPolicyNode.never)
          case CountBox(maxRetries, inner) =>
            validateUint32(maxRetries, s"$path.maxRetries").flatMap { rawMaxRetries =>
              append(inner, s"$path.inner").map { innerIndex =>
                JsPolicyNode.countBox(JsCountBoxConfig(rawMaxRetries, innerIndex))
              }
            }
          case TimeBox(limit, inner) =>
            for {
              rawLimit   <- durationToJs(limit, s"$path.limit")
              innerIndex <- append(inner, s"$path.inner")
            } yield JsPolicyNode.timeBox(JsTimeBoxConfig(rawLimit, innerIndex))
          case Clamp(minDelay, maxDelay, inner) =>
            if (minDelay > maxDelay) {
              Left(ValidationError(s"$path requires minDelay <= maxDelay, got $minDelay > $maxDelay"))
            } else {
              for {
                rawMinDelay <- durationToJs(minDelay, s"$path.minDelay")
                rawMaxDelay <- durationToJs(maxDelay, s"$path.maxDelay")
                innerIndex  <- append(inner, s"$path.inner")
              } yield JsPolicyNode.clampDelay(JsClampConfig(rawMinDelay, rawMaxDelay, innerIndex))
            }
          case AddDelay(delay, inner) =>
            for {
              rawDelay   <- durationToJs(delay, s"$path.delay")
              innerIndex <- append(inner, s"$path.inner")
            } yield JsPolicyNode.addDelay(JsAddDelayConfig(rawDelay, innerIndex))
          case Jitter(factor, inner) =>
            for {
              _          <- validatePositiveFiniteDouble(factor, s"$path.factor", allowZero = true)
              innerIndex <- append(inner, s"$path.inner")
            } yield JsPolicyNode.jitter(JsJitterConfig(factor, innerIndex))
          case FilteredOn(predicate, inner) =>
            for {
              rawPredicate <- Predicate.toJs(predicate)
              innerIndex   <- append(inner, s"$path.inner")
            } yield JsPolicyNode.filteredOn(JsFilteredConfig(rawPredicate, innerIndex))
          case AndThen(left, right) =>
            for {
              leftIndex  <- append(left, s"$path.left")
              rightIndex <- append(right, s"$path.right")
            } yield JsPolicyNode.andThen(leftIndex, rightIndex)
          case Union(left, right) =>
            for {
              leftIndex  <- append(left, s"$path.left")
              rightIndex <- append(right, s"$path.right")
            } yield JsPolicyNode.policyUnion(leftIndex, rightIndex)
          case Intersect(left, right) =>
            for {
              leftIndex  <- append(left, s"$path.left")
              rightIndex <- append(right, s"$path.right")
            } yield JsPolicyNode.policyIntersect(leftIndex, rightIndex)
        }

        built.map { node =>
          nodes.update(index, node)
          index
        }
      }

      append(policy, "policy").map(_ => JsRetryPolicyTree(nodes))
    }

    def fromJs(raw: JsRetryPolicyTree): Policy = {
      val nodes = raw.nodes
      if (nodes.isEmpty) throw new IllegalArgumentException("Retry policy must contain at least one node")

      val cache      = mutable.Map.empty[Int, Policy]
      val inProgress = mutable.Set.empty[Int]

      def requireValidIndex(index: Int, path: String): Unit =
        if (index < 0 || index >= nodes.length) {
          throw InvalidRetryShape(s"$path references policy node $index, but only ${nodes.length} nodes exist")
        }

      def build(index: Int, path: String): Policy = {
        requireValidIndex(index, path)
        cache.getOrElseUpdate(
          index, {
            if (!inProgress.add(index)) {
              throw InvalidRetryShape(s"$path contains a cycle at policy node $index")
            }

            try {
              val node = nodes(index)
              if (node == null) {
                throw InvalidRetryShape(s"$path points to a null policy node")
              }

              node.tag match {
                case "periodic" =>
                  Periodic(durationFromJs(node.asInstanceOf[JsPolicyNodeDuration].value, s"$path.delay"))
                case "exponential" =>
                  val value = node.asInstanceOf[JsPolicyNodeExponential].value
                  ensurePositiveFiniteDouble(value.factor, s"$path.factor", allowZero = false)
                  Exponential(durationFromJs(value.baseDelay, s"$path.baseDelay"), value.factor)
                case "fibonacci" =>
                  val value = node.asInstanceOf[JsPolicyNodeFibonacci].value
                  Fibonacci(
                    durationFromJs(value.first, s"$path.first"),
                    durationFromJs(value.second, s"$path.second")
                  )
                case "immediate" =>
                  Immediate
                case "never" =>
                  Never
                case "count-box" =>
                  val value = node.asInstanceOf[JsPolicyNodeCountBox].value
                  CountBox(uint32FromJs(value.maxRetries, s"$path.maxRetries"), build(value.inner, s"$path.inner"))
                case "time-box" =>
                  val value = node.asInstanceOf[JsPolicyNodeTimeBox].value
                  TimeBox(durationFromJs(value.limit, s"$path.limit"), build(value.inner, s"$path.inner"))
                case "clamp-delay" =>
                  val value    = node.asInstanceOf[JsPolicyNodeClamp].value
                  val minDelay = durationFromJs(value.minDelay, s"$path.minDelay")
                  val maxDelay = durationFromJs(value.maxDelay, s"$path.maxDelay")
                  if (minDelay > maxDelay) {
                    throw InvalidRetryShape(s"$path requires minDelay <= maxDelay, got $minDelay > $maxDelay")
                  }
                  Clamp(minDelay, maxDelay, build(value.inner, s"$path.inner"))
                case "add-delay" =>
                  val value = node.asInstanceOf[JsPolicyNodeAddDelay].value
                  AddDelay(durationFromJs(value.delay, s"$path.delay"), build(value.inner, s"$path.inner"))
                case "jitter" =>
                  val value = node.asInstanceOf[JsPolicyNodeJitter].value
                  ensurePositiveFiniteDouble(value.factor, s"$path.factor", allowZero = true)
                  Jitter(value.factor, build(value.inner, s"$path.inner"))
                case "filtered-on" =>
                  val value = node.asInstanceOf[JsPolicyNodeFiltered].value
                  FilteredOn(Predicate.fromJs(value.predicate), build(value.inner, s"$path.inner"))
                case "and-then" =>
                  val value = node.asInstanceOf[JsPolicyNodePair].value
                  AndThen(build(value._1, s"$path.left"), build(value._2, s"$path.right"))
                case "policy-union" =>
                  val value = node.asInstanceOf[JsPolicyNodePair].value
                  Union(build(value._1, s"$path.left"), build(value._2, s"$path.right"))
                case "policy-intersect" =>
                  val value = node.asInstanceOf[JsPolicyNodePair].value
                  Intersect(build(value._1, s"$path.left"), build(value._2, s"$path.right"))
                case other =>
                  throw new IllegalArgumentException(s"Unsupported retry policy node tag: $other")
              }
            } catch {
              case error: DecodeError => throw error
              case error: Throwable   =>
                throw InvalidRetryShape(
                  s"$path contains malformed policy payload: ${Option(error.getMessage).getOrElse(error.getClass.getSimpleName)}"
                )
            } finally {
              inProgress.remove(index)
            }
          }
        )
      }

      try {
        build(0, "policy.root")
      } catch {
        case error: DecodeError => throw new IllegalArgumentException(error.message, error)
      }
    }
  }

  final case class NamedPolicy(
    name: String,
    policy: Policy,
    priority: Long = 0,
    predicate: Predicate = Predicate.always
  ) {
    def priority(value: Long): NamedPolicy =
      copy(priority = value)

    def appliesWhen(value: Predicate): NamedPolicy =
      copy(predicate = value)
  }

  object NamedPolicy {
    def toJs(namedPolicy: NamedPolicy): Either[ValidationError, JsNamedRetryPolicy] =
      validateUint32(namedPolicy.priority, "namedPolicy.priority").flatMap { rawPriority =>
        for {
          rawPredicate <- Predicate.toJs(namedPolicy.predicate)
          rawPolicy    <- Policy.toJs(namedPolicy.policy)
        } yield JsNamedRetryPolicy(namedPolicy.name, rawPriority, rawPredicate, rawPolicy)
      }

    def fromJs(raw: JsNamedRetryPolicy): NamedPolicy =
      try {
        NamedPolicy(
          name = raw.name,
          policy = Policy.fromJs(raw.policy),
          priority = uint32FromJs(raw.priority, "namedPolicy.priority"),
          predicate = Predicate.fromJs(raw.predicate)
        )
      } catch {
        case error: DecodeError => throw new IllegalArgumentException(error.message, error)
      }
  }

  def named(name: String, policy: Policy): NamedPolicy =
    NamedPolicy(name = name, policy = policy)

  private[golem] def propertiesToJs(
    properties: Iterable[Property]
  ): Either[ValidationError, js.Array[js.Tuple2[String, JsPredicateValue]]] =
    sequence(properties.zipWithIndex.map { case (Property(name, value), index) =>
      PredicateValue.toJs(value, s"properties[$index].value").map(jsValue => js.Tuple2(name, jsValue))
    }).map(_.toJSArray)

  private[golem] def namedPolicyToJsOrThrow(namedPolicy: NamedPolicy): JsNamedRetryPolicy =
    eitherToRaw(NamedPolicy.toJs(namedPolicy))

  private[golem] def propertiesToJsOrThrow(
    properties: Iterable[Property]
  ): js.Array[js.Tuple2[String, JsPredicateValue]] =
    eitherToRaw(propertiesToJs(properties))

  private def eitherToRaw[A](value: Either[ValidationError, A]): A =
    value.fold(error => throw new IllegalArgumentException(error.message), identity)

  private def sequence[A](values: Iterable[Either[ValidationError, A]]): Either[ValidationError, List[A]] =
    values.foldLeft(Right(List.empty[A]): Either[ValidationError, List[A]]) { (acc, next) =>
      for {
        items <- acc
        item  <- next
      } yield items :+ item
    }

  private def validatePositiveFiniteDouble(
    value: Double,
    path: String,
    allowZero: Boolean
  ): Either[ValidationError, Unit] =
    if (!value.isFinite) {
      Left(ValidationError(s"$path must be finite, got $value"))
    } else if (allowZero && value < 0.0) {
      Left(ValidationError(s"$path must be >= 0, got $value"))
    } else if (!allowZero && value <= 0.0) {
      Left(ValidationError(s"$path must be > 0, got $value"))
    } else {
      Right(())
    }

  private def ensurePositiveFiniteDouble(value: Double, path: String, allowZero: Boolean): Unit =
    validatePositiveFiniteDouble(value, path, allowZero).fold(error => throw InvalidRetryShape(error.message), identity)

  private def validateUint32(value: Long, path: String): Either[ValidationError, Double] =
    if (value < 0L) {
      Left(ValidationError(s"$path must be >= 0, got $value"))
    } else if (value > MaxU32) {
      Left(ValidationError(s"$path must fit an unsigned 32-bit integer, got $value"))
    } else {
      Right(value.toDouble)
    }

  private def uint32FromJs(value: Double, path: String): Long =
    if (!value.isFinite || value < 0 || value > MaxU32.toDouble || value != value.floor) {
      throw InvalidRetryShape(s"$path must be a whole unsigned 32-bit integer, got $value")
    } else {
      value.toLong
    }

  private def durationToJs(duration: FiniteDuration, path: String): Either[ValidationError, js.BigInt] = {
    val nanos = durationToNanos(duration)
    if (nanos < 0) {
      Left(ValidationError(s"$path must be >= 0, got $duration"))
    } else if (nanos > MaxU64) {
      Left(ValidationError(s"$path must fit an unsigned 64-bit duration, got $duration"))
    } else {
      Right(js.BigInt(nanos.toString))
    }
  }

  private def durationFromJs(durationNanos: js.BigInt, path: String): FiniteDuration = {
    val nanos = BigInt(durationNanos.toString)
    if (nanos < 0) {
      throw InvalidRetryShape(s"$path must be >= 0, got $nanos ns")
    } else if (nanos > BigInt(Long.MaxValue)) {
      throw InvalidRetryShape(s"$path exceeds Scala FiniteDuration range: $nanos ns")
    } else {
      FiniteDuration(nanos.longValue, TimeUnit.NANOSECONDS)
    }
  }

  private def durationToNanos(duration: FiniteDuration): BigInt = {
    val multiplier = duration.unit match {
      case TimeUnit.NANOSECONDS  => BigInt(1)
      case TimeUnit.MICROSECONDS => BigInt(1000)
      case TimeUnit.MILLISECONDS => BigInt(1000000)
      case TimeUnit.SECONDS      => BigInt(1000000000)
      case TimeUnit.MINUTES      => BigInt(60000000000L)
      case TimeUnit.HOURS        => BigInt(3600000000000L)
      case TimeUnit.DAYS         => BigInt(86400000000000L)
    }

    BigInt(duration.length) * multiplier
  }

  private val MinS64: BigInt = BigInt(Long.MinValue)
  private val MaxS64: BigInt = BigInt(Long.MaxValue)
  private val MaxU32: Long   = 0xffffffffL
  private val MaxU64: BigInt = (BigInt(1) << 64) - 1
}
