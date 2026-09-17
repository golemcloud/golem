/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.host

import scala.collection.mutable.ListBuffer
import scala.concurrent.duration._
import scala.concurrent.{Future, Promise}
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js
import scala.scalajs.js.timers.setTimeout
import scala.util.control.NonFatal

private[host] object LocalRetry {
  private sealed trait State
  private final case class Counter(value: Long)                                 extends State
  private case object Terminal                                                  extends State
  private final case class Wrapper(inner: State)                                extends State
  private final case class CountBox(attempts: Long, inner: State)               extends State
  private final case class AndThen(left: State, right: State, onRight: Boolean) extends State
  private final case class Pair(left: State, right: State)                      extends State

  private sealed trait Verdict
  private final case class RetryAfter(delay: FiniteDuration) extends Verdict
  private case object GiveUp                                 extends Verdict
  private case object EvaluationError                        extends Verdict

  private[host] trait Runtime {
    def nowNanos(): Long
    def sleep(delay: FiniteDuration): Future[Unit]
    def random(): Double
  }

  private object DefaultRuntime extends Runtime {
    override def nowNanos(): Long = (js.Date.now() * 1000000.0).toLong

    override def sleep(delay: FiniteDuration): Future[Unit] = {
      def loop(remaining: FiniteDuration): Future[Unit] = {
        val current = nextTimerDelay(remaining)
        val result  = Promise[Unit]()
        setTimeout(current)(result.success(()))
        result.future.flatMap { _ =>
          if (remaining > current) loop(remaining - current) else Future.successful(())
        }
      }
      loop(delay)
    }

    override def random(): Double = Math.random()
  }

  private[host] def nextTimerDelay(delay: FiniteDuration): FiniteDuration = delay.min(MaxTimerDelay)

  def retry[A](
    policy: Retry.Policy,
    properties: Throwable => Iterable[Retry.Property],
    operation: () => Future[A]
  ): Future[A] =
    Retry.Policy.toJs(policy) match {
      case Left(error) => Future.failed(new IllegalArgumentException(error.message))
      case Right(_)    => retryWith(policy, properties, operation, DefaultRuntime)
    }

  private[host] def retryWith[A](
    policy: Retry.Policy,
    properties: Throwable => Iterable[Retry.Property],
    operation: () => Future[A],
    runtime: Runtime
  ): Future[A] = {
    val startedAt = runtime.nowNanos()

    def invoke(): Future[A] =
      try operation()
      catch {
        case NonFatal(error) => Future.failed(error)
      }

    def loop(state: State): Future[A] =
      invoke().recoverWith { case error =>
        try {
          val projected            = properties(error).iterator.map(property => property.name -> property.value).toMap
          val elapsed              = nanos(runtime.nowNanos() - startedAt)
          val (nextState, verdict) = step(policy, state, elapsed, projected, runtime)
          verdict match {
            case RetryAfter(delay)        => runtime.sleep(delay).flatMap(_ => loop(nextState))
            case GiveUp | EvaluationError => Future.failed(error)
          }
        } catch {
          case NonFatal(_) => Future.failed(error)
        }
      }

    loop(initialState(policy))
  }

  private def initialState(policy: Retry.Policy): State =
    policy match {
      case Retry.Policy.Periodic(_) | Retry.Policy.Exponential(_, _) | Retry.Policy.Fibonacci(_, _) |
          Retry.Policy.Immediate =>
        Counter(0)
      case Retry.Policy.Never                  => Terminal
      case Retry.Policy.CountBox(_, inner)     => CountBox(0, initialState(inner))
      case Retry.Policy.TimeBox(_, inner)      => Wrapper(initialState(inner))
      case Retry.Policy.Clamp(_, _, inner)     => Wrapper(initialState(inner))
      case Retry.Policy.AddDelay(_, inner)     => Wrapper(initialState(inner))
      case Retry.Policy.Jitter(_, inner)       => Wrapper(initialState(inner))
      case Retry.Policy.FilteredOn(_, inner)   => Wrapper(initialState(inner))
      case Retry.Policy.AndThen(left, right)   => AndThen(initialState(left), initialState(right), onRight = false)
      case Retry.Policy.Union(left, right)     => Pair(initialState(left), initialState(right))
      case Retry.Policy.Intersect(left, right) => Pair(initialState(left), initialState(right))
    }

  private def step(
    policy: Retry.Policy,
    state: State,
    elapsed: FiniteDuration,
    properties: Map[String, Retry.PredicateValue],
    runtime: Runtime
  ): (State, Verdict) =
    (policy, state) match {
      case (Retry.Policy.Periodic(delay), Counter(counter)) =>
        Counter(increment(counter)) -> RetryAfter(delay)
      case (Retry.Policy.Exponential(baseDelay, factor), Counter(counter)) =>
        Counter(increment(counter)) -> RetryAfter(scale(baseDelay, Math.pow(factor, counter.toDouble)))
      case (Retry.Policy.Fibonacci(first, second), Counter(counter)) =>
        Counter(increment(counter)) -> RetryAfter(fibonacci(first, second, increment(counter)))
      case (Retry.Policy.Immediate, Counter(counter)) =>
        Counter(increment(counter)) -> RetryAfter(Duration.Zero)
      case (Retry.Policy.Never, Terminal)                                                       => Terminal -> GiveUp
      case (Retry.Policy.CountBox(maxRetries, inner), current @ CountBox(attempts, innerState)) =>
        if (attempts >= maxRetries) current -> GiveUp
        else {
          val (nextInner, verdict) = step(inner, innerState, elapsed, properties, runtime)
          CountBox(increment(attempts), nextInner) -> verdict
        }
      case (Retry.Policy.TimeBox(limit, _), current @ Wrapper(_)) if elapsed >= limit => current -> GiveUp
      case (Retry.Policy.TimeBox(_, inner), Wrapper(innerState))                      =>
        val (nextInner, verdict) = step(inner, innerState, elapsed, properties, runtime)
        Wrapper(nextInner) -> verdict
      case (Retry.Policy.Clamp(minDelay, maxDelay, inner), Wrapper(innerState)) =>
        val (nextInner, verdict) = step(inner, innerState, elapsed, properties, runtime)
        Wrapper(nextInner) -> mapDelay(verdict)(delay => delay.max(minDelay).min(maxDelay))
      case (Retry.Policy.AddDelay(delay, inner), Wrapper(innerState)) =>
        val (nextInner, verdict) = step(inner, innerState, elapsed, properties, runtime)
        Wrapper(nextInner) -> mapDelay(verdict)(saturatingAdd(_, delay))
      case (Retry.Policy.Jitter(factor, inner), Wrapper(innerState)) =>
        val (nextInner, verdict) = step(inner, innerState, elapsed, properties, runtime)
        Wrapper(nextInner) -> mapDelay(verdict) { delay =>
          if (factor == 0.0) delay else saturatingAdd(delay, scale(delay, runtime.random() * factor))
        }
      case (Retry.Policy.FilteredOn(predicate, inner), current @ Wrapper(innerState)) =>
        evaluate(predicate, properties) match {
          case Left(_)      => current -> EvaluationError
          case Right(false) => current -> GiveUp
          case Right(true)  =>
            val (nextInner, verdict) = step(inner, innerState, elapsed, properties, runtime)
            Wrapper(nextInner) -> verdict
        }
      case (Retry.Policy.AndThen(leftPolicy, rightPolicy), AndThen(left, right, false)) =>
        val (nextLeft, leftVerdict) = step(leftPolicy, left, elapsed, properties, runtime)
        leftVerdict match {
          case retry: RetryAfter => AndThen(nextLeft, right, onRight = false) -> retry
          case GiveUp            =>
            val (nextRight, verdict) = step(rightPolicy, right, elapsed, properties, runtime)
            AndThen(nextLeft, nextRight, onRight = true) -> verdict
          case EvaluationError => AndThen(nextLeft, right, onRight = false) -> EvaluationError
        }
      case (Retry.Policy.AndThen(_, rightPolicy), AndThen(left, right, true)) =>
        val (nextRight, verdict) = step(rightPolicy, right, elapsed, properties, runtime)
        AndThen(left, nextRight, onRight = true) -> verdict
      case (Retry.Policy.Union(leftPolicy, rightPolicy), Pair(left, right)) =>
        val (nextLeft, leftVerdict)   = step(leftPolicy, left, elapsed, properties, runtime)
        val (nextRight, rightVerdict) = step(rightPolicy, right, elapsed, properties, runtime)
        Pair(nextLeft, nextRight) -> union(leftVerdict, rightVerdict)
      case (Retry.Policy.Intersect(leftPolicy, rightPolicy), Pair(left, right)) =>
        val (nextLeft, leftVerdict)   = step(leftPolicy, left, elapsed, properties, runtime)
        val (nextRight, rightVerdict) = step(rightPolicy, right, elapsed, properties, runtime)
        Pair(nextLeft, nextRight) -> intersect(leftVerdict, rightVerdict)
      case _ => initialState(policy) -> GiveUp
    }

  private def evaluate(
    predicate: Retry.Predicate,
    properties: Map[String, Retry.PredicateValue]
  ): Either[Unit, Boolean] =
    try {
      Right(matchesStrict(predicate, properties))
    } catch {
      case EvaluationFailure => Left(())
    }

  private def matchesStrict(predicate: Retry.Predicate, properties: Map[String, Retry.PredicateValue]): Boolean =
    predicate match {
      case Retry.Predicate.Eq(property, value)            => compare(required(properties, property), value) == 0
      case Retry.Predicate.Neq(property, value)           => compare(required(properties, property), value) != 0
      case Retry.Predicate.Gt(property, value)            => compare(required(properties, property), value) > 0
      case Retry.Predicate.Gte(property, value)           => compare(required(properties, property), value) >= 0
      case Retry.Predicate.Lt(property, value)            => compare(required(properties, property), value) < 0
      case Retry.Predicate.Lte(property, value)           => compare(required(properties, property), value) <= 0
      case Retry.Predicate.Exists(property)               => properties.contains(property)
      case Retry.Predicate.OneOf(property, values)        => oneOf(required(properties, property), values)
      case Retry.Predicate.MatchesGlob(property, pattern) => glob(pattern, asText(required(properties, property)))
      case Retry.Predicate.StartsWith(property, prefix)   => asText(required(properties, property)).startsWith(prefix)
      case Retry.Predicate.Contains(property, substring)  => asText(required(properties, property)).contains(substring)
      case Retry.Predicate.And(left, right)               => matchesStrict(left, properties) && matchesStrict(right, properties)
      case Retry.Predicate.Or(left, right)                => matchesStrict(left, properties) || matchesStrict(right, properties)
      case Retry.Predicate.Not(inner)                     => !matchesStrict(inner, properties)
      case Retry.Predicate.Always                         => true
      case Retry.Predicate.Never                          => false
    }

  private case object EvaluationFailure extends Throwable with scala.util.control.NoStackTrace

  private def required(properties: Map[String, Retry.PredicateValue], name: String): Retry.PredicateValue =
    properties.getOrElse(name, throw EvaluationFailure)

  private def compare(actual: Retry.PredicateValue, expected: Retry.PredicateValue): Int =
    (actual, expected) match {
      case (Retry.PredicateValue.Integer(left), Retry.PredicateValue.Integer(right))           => left.compare(right)
      case (Retry.PredicateValue.Text(left), Retry.PredicateValue.Text(right))                 => compareBytes(utf8(left), utf8(right))
      case (Retry.PredicateValue.BooleanValue(left), Retry.PredicateValue.BooleanValue(right)) => left.compareTo(right)
      case (Retry.PredicateValue.Text(left), Retry.PredicateValue.Integer(right))              =>
        try {
          if (!isSignedAsciiInteger(left)) throw EvaluationFailure
          val integer = BigInt(left)
          if (integer < MinS64 || integer > MaxS64) throw EvaluationFailure
          integer.compare(right)
        } catch { case _: NumberFormatException => throw EvaluationFailure }
      case (Retry.PredicateValue.Integer(left), Retry.PredicateValue.Text(right)) => left.toString.compareTo(right)
      case _                                                                      => throw EvaluationFailure
    }

  private def isSignedAsciiInteger(value: String): Boolean = {
    val firstDigit = if (value.startsWith("+") || value.startsWith("-")) 1 else 0
    value.length > firstDigit && value.substring(firstDigit).forall(character => character >= '0' && character <= '9')
  }

  private def oneOf(actual: Retry.PredicateValue, values: List[Retry.PredicateValue]): Boolean = {
    var hadError = false
    val matched  = values.exists { value =>
      try {
        compare(actual, value) == 0
      } catch {
        case EvaluationFailure => hadError = true; false
      }
    }
    if (matched) true else if (hadError) throw EvaluationFailure else false
  }

  private def compareBytes(left: Array[Int], right: Array[Int]): Int = {
    var index = 0
    while (index < left.length && index < right.length) {
      val compared = left(index).compare(right(index))
      if (compared != 0) return compared
      index += 1
    }
    left.length.compare(right.length)
  }

  private def asText(value: Retry.PredicateValue): String =
    value match {
      case Retry.PredicateValue.Text(text)       => text
      case Retry.PredicateValue.Integer(integer) => integer.toString
      case Retry.PredicateValue.BooleanValue(_)  => throw EvaluationFailure
    }

  private def glob(pattern: String, value: String): Boolean = {
    val source = pattern.iterator.map {
      case '*'                                                  => ".*"
      case '?'                                                  => "."
      case character if RegexMetacharacters.contains(character) => s"\\$character"
      case character                                            => character.toString
    }.mkString
    new js.RegExp(s"^$source$$").test(value)
  }

  private def utf8(value: String): Array[Int] = {
    val result = ListBuffer.empty[Int]
    var index  = 0
    while (index < value.length) {
      val first     = value.charAt(index).toInt
      val codePoint =
        if (first >= 0xd800 && first <= 0xdbff && index + 1 < value.length) {
          val second = value.charAt(index + 1).toInt
          if (second >= 0xdc00 && second <= 0xdfff) {
            index += 1
            0x10000 + ((first - 0xd800) << 10) + second - 0xdc00
          } else first
        } else first
      if (codePoint <= 0x7f) result += codePoint
      else if (codePoint <= 0x7ff) {
        result += (0xc0 | (codePoint >> 6)); result += (0x80 | (codePoint & 0x3f))
      } else if (codePoint <= 0xffff) {
        result += (0xe0 | (codePoint >> 12)); result += (0x80 | ((codePoint >> 6) & 0x3f));
        result += (0x80 | (codePoint & 0x3f))
      } else {
        result += (0xf0 | (codePoint >> 18)); result += (0x80 | ((codePoint >> 12) & 0x3f));
        result += (0x80 | ((codePoint >> 6) & 0x3f)); result += (0x80 | (codePoint & 0x3f))
      }
      index += 1
    }
    result.toArray
  }

  private def union(left: Verdict, right: Verdict): Verdict =
    (left, right) match {
      case (EvaluationError, _) | (_, EvaluationError) => EvaluationError
      case (RetryAfter(a), RetryAfter(b))              => RetryAfter(a.min(b))
      case (retry: RetryAfter, GiveUp)                 => retry
      case (GiveUp, retry: RetryAfter)                 => retry
      case (GiveUp, GiveUp)                            => GiveUp
    }

  private def intersect(left: Verdict, right: Verdict): Verdict =
    (left, right) match {
      case (EvaluationError, _) | (_, EvaluationError) => EvaluationError
      case (RetryAfter(a), RetryAfter(b))              => RetryAfter(a.max(b))
      case _                                           => GiveUp
    }

  private def mapDelay(verdict: Verdict)(f: FiniteDuration => FiniteDuration): Verdict =
    verdict match {
      case RetryAfter(delay) => RetryAfter(f(delay))
      case GiveUp            => GiveUp
      case EvaluationError   => EvaluationError
    }

  private def increment(value: Long): Long = if (value == Long.MaxValue) value else value + 1

  private def fibonacci(first: FiniteDuration, second: FiniteDuration, nth: Long): FiniteDuration =
    if (nth <= 1) first
    else if (nth == 2) second
    else {
      var left  = first
      var right = second
      var n     = 3L
      while (n <= nth && right != MaxDuration) {
        val next = saturatingAdd(left, right)
        left = right
        right = next
        n += 1
      }
      right
    }

  private def scale(duration: FiniteDuration, factor: Double): FiniteDuration =
    if (!factor.isFinite || factor <= 0.0 || duration.length == 0) Duration.Zero
    else {
      val scaled = duration.toNanos.toDouble * factor
      if (!scaled.isFinite || scaled >= Long.MaxValue.toDouble) MaxDuration
      else nanos(Math.round(scaled))
    }

  private def saturatingAdd(left: FiniteDuration, right: FiniteDuration): FiniteDuration =
    if (left.toNanos > Long.MaxValue - right.toNanos) MaxDuration else nanos(left.toNanos + right.toNanos)

  private def nanos(value: Long): FiniteDuration = math.max(0L, value).nanos

  private val MinS64              = BigInt(Long.MinValue)
  private val MaxS64              = BigInt(Long.MaxValue)
  private val MaxDuration         = Long.MaxValue.nanos
  private val MaxTimerDelay       = Int.MaxValue.millis
  private val RegexMetacharacters = "\\^$.*+?()[]{}|"
}
