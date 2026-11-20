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

package golem

import zio.blocks.schema.Schema

import java.time.Instant
import java.util.UUID

/**
 * The identity of one principal: an issuer and a subject.
 *
 * A reference is data: holding one grants nothing. Two references name the same
 * principal exactly when both strings are equal, byte for byte. The text form
 * is `("<issuer>","<subject>")`; [[PrincipalRef.parse]] refuses every other
 * spelling. As method data it is the named record `principal-ref { issuer,
 * subject }`.
 */
final case class PrincipalRef(issuer: String, subject: String) {

  /** The canonical text form `("<issuer>","<subject>")`. */
  override def toString: String =
    "(" + PrincipalRef.printString(issuer) + "," + PrincipalRef.printString(subject) + ")"
}

object PrincipalRef {
  implicit val schema: Schema[PrincipalRef] = Schema.derived

  /**
   * Reads the canonical text form. Fails with an [[IllegalArgumentException]]
   * on any other spelling.
   */
  def parse(text: String): PrincipalRef =
    parseEither(text) match {
      case Right(ref)    => ref
      case Left(message) => throw new IllegalArgumentException(message)
    }

  /** Like [[parse]], returning the failure instead of throwing it. */
  def parseEither(text: String): Either[String, PrincipalRef] = {
    val parser = new Parser(text)
    try {
      parser.expect('(')
      val issuer = parser.string()
      parser.expect(',')
      val subject = parser.string()
      parser.expect(')')
      parser.end()
      Right(PrincipalRef(issuer, subject))
    } catch {
      case failure: ParseFailure => Left(failure.getMessage)
    }
  }

  private final class ParseFailure(message: String) extends RuntimeException(message)

  /** Prints a string the way RFC 8785 serializes JSON strings. */
  private[golem] def printString(value: String): String = {
    val out = new java.lang.StringBuilder("\"")
    var i   = 0
    while (i < value.length) {
      val c = value.charAt(i)
      c match {
        case '"'           => out.append("\\\"")
        case '\\'          => out.append("\\\\")
        case '\b'          => out.append("\\b")
        case '\t'          => out.append("\\t")
        case '\n'          => out.append("\\n")
        case '\f'          => out.append("\\f")
        case '\r'          => out.append("\\r")
        case _ if c < 0x20 =>
          out.append("\\u00")
          out.append(Character.forDigit((c >> 4) & 0xf, 16))
          out.append(Character.forDigit(c & 0xf, 16))
        case _ => out.append(c)
      }
      i += 1
    }
    out.append('"').toString
  }

  private final class Parser(text: String) {
    private var position = 0

    private def fail(message: String): Nothing =
      throw new ParseFailure(s"invalid principal reference text: $message at character $position")

    def expect(expected: Char): Unit = {
      if (position >= text.length || text.charAt(position) != expected) fail(s"expected `$expected`")
      position += 1
    }

    def end(): Unit =
      if (position < text.length) fail("trailing text")

    def string(): String = {
      expect('"')
      val value = new java.lang.StringBuilder
      var done  = false
      while (!done) {
        if (position >= text.length) fail("unterminated string")
        val c = text.charAt(position)
        position += 1
        if (c == '"') done = true
        else if (c == '\\') value.append(escape())
        else if (c < 0x20) fail("unescaped control character")
        else value.append(c)
      }
      value.toString
    }

    /**
     * Reads the escape after a backslash. Only the escapes the printer writes
     * are accepted.
     */
    private def escape(): Char = {
      if (position >= text.length) fail("non-canonical escape")
      val c = text.charAt(position)
      position += 1
      c match {
        case '"'  => '"'
        case '\\' => '\\'
        case 'b'  => '\b'
        case 't'  => '\t'
        case 'n'  => '\n'
        case 'f'  => '\f'
        case 'r'  => '\r'
        case 'u'  =>
          val digits = if (position + 4 <= text.length) text.substring(position, position + 4) else ""
          if (digits.length != 4 || !digits.forall(d => (d >= '0' && d <= '9') || (d >= 'a' && d <= 'f')))
            fail("non-canonical escape")
          val code = Integer.parseInt(digits, 16)
          // A control character has the short escape when it has one, else a
          // four-digit escape; every other character is written as itself.
          if (code >= 0x20 || code == 0x08 || code == 0x09 || code == 0x0a || code == 0x0c || code == 0x0d)
            fail("non-canonical escape")
          position += 4
          code.toChar
        case _ => fail("non-canonical escape")
      }
    }
  }
}

/** The five kinds of principal. */
sealed abstract class PrincipalKind(val name: String) extends Product with Serializable {
  override def toString: String = name
}

object PrincipalKind {
  case object User      extends PrincipalKind("user")
  case object Account   extends PrincipalKind("account")
  case object Agent     extends PrincipalKind("agent")
  case object Tool      extends PrincipalKind("tool")
  case object Anonymous extends PrincipalKind("anonymous")
}

/**
 * One side of a call: a user, an account, an agent, a tool or nobody.
 *
 * A principal is a live value, not data: it has no schema and cannot be a
 * method parameter, a result or part of an agent id. Store `principal.ref`
 * instead.
 */
sealed trait Principal extends Product with Serializable {
  def kind: PrincipalKind

  /** The reference of this principal, or none for anonymous. */
  def ref: Option[PrincipalRef]

  /**
   * Whether this principal is the one `other` names. An anonymous principal is
   * the same as nothing.
   */
  def isSameAs(other: Principal): Boolean = (ref, other.ref) match {
    case (Some(a), Some(b)) => a == b
    case _                  => false
  }

  def isSameAs(other: PrincipalRef): Boolean = ref.contains(other)

  /** `"<kind> <ref text>"`, or `"anonymous"`. It never includes claims. */
  override def toString: String = ref match {
    case Some(r) => kind.name + " " + r.toString
    case None    => "anonymous"
  }
}

object Principal {

  /** A person who signed in through an identity provider. */
  final case class User(pair: PrincipalRef, claimsJson: String, authenticatedAt: Instant) extends Principal {
    def kind: PrincipalKind       = PrincipalKind.User
    def ref: Option[PrincipalRef] = Some(pair)
    def issuer: String            = pair.issuer
    def subject: String           = pair.subject
    override def toString: String = super.toString
  }

  /** A Golem account, as seen through the CLI, the REST API or a token. */
  final case class Account(pair: PrincipalRef, accountId: UUID, authentication: String) extends Principal {
    def kind: PrincipalKind       = PrincipalKind.Account
    def ref: Option[PrincipalRef] = Some(pair)
    override def toString: String = super.toString
  }

  /** A running agent that made a call. */
  final case class Agent(
    pair: PrincipalRef,
    agentId: AgentId,
    agentType: String,
    environmentId: UUID
  ) extends Principal {
    def kind: PrincipalKind       = PrincipalKind.Agent
    def ref: Option[PrincipalRef] = Some(pair)
    override def toString: String = super.toString
  }

  /** A tool that made a call. */
  final case class Tool(pair: PrincipalRef, environmentId: UUID, toolName: String) extends Principal {
    def kind: PrincipalKind       = PrincipalKind.Tool
    def ref: Option[PrincipalRef] = Some(pair)
    override def toString: String = super.toString
  }

  /** Nobody: an unauthenticated caller. It has no reference. */
  case object Anonymous extends Principal {
    def kind: PrincipalKind       = PrincipalKind.Anonymous
    def ref: Option[PrincipalRef] = None
    override def toString: String = "anonymous"
  }
}

/**
 * Who made a token and which token it is. The id is the token's own, never its
 * secret.
 */
final case class TokenProvenance(id: UUID, maker: Option[PrincipalRef])

/**
 * Who caused the running invocation: the `original` principal that started the
 * chain, the `immediate` principal that made this call, and the provenance of
 * the tokens on the way.
 */
final case class Caller(original: Principal, immediate: Principal, tokens: List[TokenProvenance] = Nil)

object Caller {

  /** A caller with no identity: anonymous in both roles and no tokens. */
  val anonymous: Caller = Caller(Principal.Anonymous, Principal.Anonymous, Nil)
}

/**
 * What the host tells an invocation about itself. The SDK injects it into an
 * agent constructor, a method, a tool or a tool middleware that declares an
 * `InvocationContext` parameter. It is not part of the input schema and callers
 * never supply it.
 *
 * The host is asked for the caller on first use only, and only during the call:
 * once the call has ended the host holds another invocation's caller, so a
 * context that has not read it by then refuses to, and a context that has keeps
 * the value it read.
 */
final class InvocationContext private (read: () => Caller) {
  private var value: Caller  = null
  private var ended: Boolean = false

  /**
   * Who caused the running invocation. Read from the host on first use, during
   * the call; a first read after the call has ended throws.
   */
  def caller: Caller = {
    if (value == null) {
      if (ended)
        throw new IllegalStateException(
          "this InvocationContext belongs to a call that has ended; store ctx.caller.original.ref instead"
        )
      value = read()
    }
    value
  }

  /**
   * Marks the call as over: the SDK calls it when the invocation returns. A
   * call that returned a stream ends with `returnedStream`: its producer runs
   * after the return, so the caller is read now, while the call still holds it,
   * and kept for the producer.
   */
  private[golem] def end(returnedStream: Boolean = false): Unit = {
    if (returnedStream && value == null && !ended) value = read()
    ended = true
  }

  override def toString: String = "InvocationContext"
}

object InvocationContext {

  /** A context whose caller is produced on first use. */
  def lazily(read: => Caller): InvocationContext = new InvocationContext(() => read)

  /** The context of an invocation made by `caller`. */
  def of(caller: Caller): InvocationContext = new InvocationContext(() => caller)

  /** The context of an anonymous invocation. */
  def anonymous: InvocationContext = of(Caller.anonymous)
}

/**
 * Rule 8: a [[Principal]], a [[Caller]] and a [[TokenProvenance]] are live
 * values, never data. The build refuses them in a signature, a result, an id
 * and a value type.
 */
private[golem] object LiveValues {
  val Message: String =
    "Principal and Caller cannot appear in a method, constructor or tool signature, or inside a value type. " +
      "Read the caller with ctx.caller (the golem.InvocationContext parameter), " +
      "and store ctx.caller.original.ref as a PrincipalRef."

  /** The fully qualified names of the live value types. */
  val TypeNames: Set[String] = Set(
    "golem.Principal",
    "golem.Principal.User",
    "golem.Principal.Account",
    "golem.Principal.Agent",
    "golem.Principal.Tool",
    "golem.Principal.Anonymous",
    "golem.Principal.Anonymous$",
    "golem.Caller",
    "golem.TokenProvenance"
  )
}
