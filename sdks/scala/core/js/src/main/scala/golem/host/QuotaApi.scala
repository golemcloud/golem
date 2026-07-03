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
import golem.schema.{
  FromSchema,
  FromSchemaError,
  GuestQuotaTokenHandle,
  IntoSchema,
  QuotaTokenSpec,
  SchemaGraph,
  SchemaType,
  SchemaTypeBody,
  SchemaValue
}

import scala.collection.immutable.ListMap
import scala.concurrent.{ExecutionContext, Future}
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

/**
 * Scala.js facade for `golem:quota/types@1.5.0`.
 *
 * The `quota-token` capability is an opaque, unforgeable, owned resource
 * defined in `golem:core/types@2.0.0`. Guest code can only hold and move the
 * handle: it cannot read the token's internals, serialize it, or forge one. The
 * quota interface exposes free functions over such a handle plus the
 * `reservation` resource:
 * {{{
 *   record failed-reservation { estimated-wait-nanos: option<u64> }
 *   resource reservation { commit: static func(this: reservation, used: u64) }
 *   new-token: func(resource-name: string, expected-use: u64) -> quota-token;
 *   reserve:   func(token: borrow<quota-token>, amount: u64)
 *                -> result<reservation, failed-reservation>;
 *   split:     func(token: borrow<quota-token>, child-expected-use: u64) -> quota-token;
 *   merge:     func(token: borrow<quota-token>, other: quota-token);
 * }}}
 */
object QuotaApi {

  // --- WIT: failed-reservation record ---

  /**
   * Returned when a reservation cannot be granted (enforcement policy =
   * `reject`).
   */
  final case class FailedReservation(
    /**
     * Estimated wait time in nanoseconds; only set for rate-limited resources.
     */
    estimatedWaitNanos: Option[BigInt]
  )

  // --- WIT: reservation resource ---

  /**
   * A short-lived capability representing a pending resource consumption.
   *
   * Dropping without calling [[commit]] is equivalent to committing zero usage.
   */
  final class Reservation private[golem] (private[golem] val underlying: JsReservation) {

    /**
     * Commit actual usage.
     *   - `used` < reserved → unused capacity is returned to the pool.
     *   - `used` > reserved → excess is deducted as "debt" from the token's
     *     allocation.
     */
    def commit(used: BigInt): Unit =
      JsReservationStatic.commit(underlying, js.BigInt(used.toString))
  }

  // --- WIT: quota-token resource ---

  /**
   * An unforgeable capability granting the right to consume a named resource.
   *
   * A `QuotaToken` holds only an opaque, affine handle to the owned host
   * resource: it carries no readable content and can be transferred to exactly
   * one destination (an RPC argument, a method return value, ...). Once
   * transferred the token can no longer be used; [[split]] first if you need to
   * both keep and send a capability.
   *
   * Typical usage:
   * {{{
   *   val token = QuotaToken("openai-tokens", BigInt(1000))
   *   token.reserve(BigInt(500)) match {
   *     case Right(reservation) =>
   *       val used = callExternalApi()
   *       reservation.commit(used)
   *     case Left(err) =>
   *       println(s"quota unavailable: $err")
   *   }
   * }}}
   */
  final class QuotaToken private[golem] (private[golem] val handle: GuestQuotaTokenHandle) {

    /**
     * Reserve `amount` units from the local allocation.
     *
     * Blocks internally until capacity is available or the resource's
     * enforcement action fires. Returns `Right(reservation)` on success, or
     * `Left(FailedReservation)` when the enforcement policy is `reject`. For
     * `throttle` / `terminate` policies the call suspends or terminates the
     * agent before returning.
     *
     * Traps if this token has already been transferred.
     */
    def reserve(amount: BigInt): Either[FailedReservation, Reservation] =
      handle.withHandle { raw =>
        try
          Right(
            new Reservation(QuotaModule.reserve(raw.asInstanceOf[JsQuotaTokenResource], js.BigInt(amount.toString)))
          )
        catch {
          case e: js.JavaScriptException =>
            val rawErr        = e.exception.asInstanceOf[JsFailedReservation]
            val estimatedWait = rawErr.estimatedWaitNanos.toOption.map(bi => BigInt(bi.toString))
            Left(FailedReservation(estimatedWait))
        }
      }
        .getOrElse(throw new IllegalStateException(TOKEN_CONSUMED))

    /**
     * Split off a child token with `childExpectedUse` units of expected-use.
     *
     *   - The parent's expected-use is reduced by `childExpectedUse`.
     *   - Credits are divided proportionally between parent and child.
     *
     * Traps if `childExpectedUse` exceeds the parent's current expected-use, or
     * if this token has already been transferred.
     */
    def split(childExpectedUse: BigInt): QuotaToken =
      handle.withHandle { raw =>
        new QuotaToken(
          GuestQuotaTokenHandle.fromRaw(
            QuotaModule.split(raw.asInstanceOf[JsQuotaTokenResource], js.BigInt(childExpectedUse.toString))
          )
        )
      }
        .getOrElse(throw new IllegalStateException(TOKEN_CONSUMED))

    /**
     * Merge `other` back into this token.
     *
     * Combines expected-use and credits. `other` is consumed by this call and
     * must not be used afterwards; this token remains usable.
     *
     * Traps if the tokens refer to different resources, or if either token has
     * already been transferred.
     */
    def merge(other: QuotaToken): Unit = {
      // Reject merging a token into itself before taking any handle, so a shared
      // handle is not consumed by the receiver and then read again as `other`.
      if (other.handle eq this.handle)
        throw new IllegalArgumentException("cannot merge a quota token with itself")
      // Borrow this token first (it is a `borrow<quota-token>` and stays usable);
      // a consumed receiver must not consume `other`.
      handle.withHandle { thisRaw =>
        other.handle.take() match {
          case Some(otherRaw) =>
            QuotaModule.merge(
              thisRaw.asInstanceOf[JsQuotaTokenResource],
              otherRaw.asInstanceOf[JsQuotaTokenResource]
            )
          case None => throw new IllegalStateException(TOKEN_CONSUMED)
        }
      }
        .getOrElse(throw new IllegalStateException(TOKEN_CONSUMED))
    }

    /**
     * Reserve `amount` units, run `f`, then commit the actual usage returned by
     * `f`. Commits zero usage on failure and re-throws.
     */
    def withReservation[T](amount: BigInt)(
      f: Reservation => Future[(BigInt, T)]
    ): Future[Either[FailedReservation, T]] =
      QuotaApi.withReservation(this, amount)(f)
  }

  object QuotaToken {

    /**
     * Request a quota capability for the named resource.
     *
     * @param resourceName
     *   The resource name as declared in the manifest.
     * @param expectedUse
     *   Expected units per reservation; used to derive the credit rate and
     *   max-credit for fair scheduling.
     */
    def apply(resourceName: String, expectedUse: BigInt): QuotaToken =
      new QuotaToken(
        GuestQuotaTokenHandle.fromRaw(QuotaModule.newToken(resourceName, js.BigInt(expectedUse.toString)))
      )

    /**
     * Automatic serialization for RPC: a `QuotaToken` is a schema-native
     * capability node (`golem:core/types@2.0.0` `quota-token` /
     * `quota-token-handle`), not a plain record. The owned handle is moved into
     * the value tree when it is encoded, so a token can be sent exactly once
     * and cannot be forged from data.
     */
    implicit val intoSchema: IntoSchema[QuotaToken] =
      new IntoSchema[QuotaToken] {
        override lazy val graph: SchemaGraph =
          SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.QuotaTokenType(QuotaTokenSpec())))

        override def toValue(token: QuotaToken): SchemaValue =
          SchemaValue.QuotaTokenHandle(token.handle)
      }

    implicit val fromSchema: FromSchema[QuotaToken] =
      new FromSchema[QuotaToken] {
        override def fromValue(value: SchemaValue): Either[FromSchemaError, QuotaToken] =
          value match {
            case SchemaValue.QuotaTokenHandle(h) => Right(new QuotaToken(h))
            case other                           =>
              Left(FromSchemaError(s"expected quota-token handle for QuotaToken, got $other"))
          }
      }
  }

  private val TOKEN_CONSUMED =
    "quota token has already been transferred and can no longer be used; split the token first if " +
      "you need to both keep and send a capability"

  private implicit val ec: ExecutionContext = ExecutionContext.global

  /**
   * Reserve `amount` units from `token`, run `f`, then commit the actual usage
   * returned by `f`. Commits zero usage on failure and re-throws, ensuring
   * unused capacity is always returned to the pool.
   *
   * Returns `Future(Left(FailedReservation))` if the reservation could not be
   * granted, or `Future(Right(value))` on success.
   *
   * {{{
   *   val result = withReservation(token, BigInt(500)) { reservation =>
   *     Future {
   *       val data = callExternalApi()
   *       (data.tokensUsed, data)
   *     }
   *   }
   * }}}
   */
  def withReservation[T](token: QuotaToken, amount: BigInt)(
    f: Reservation => Future[(BigInt, T)]
  ): Future[Either[FailedReservation, T]] =
    token.reserve(amount) match {
      case Left(err)          => Future.successful(Left(err))
      case Right(reservation) =>
        f(reservation).map { case (used, value) =>
          reservation.commit(used)
          Right(value)
        }.recoverWith { case e: Throwable =>
          reservation.commit(BigInt(0))
          Future.failed(e)
        }
    }

  @js.native
  @JSImport("golem:quota/types@1.5.0", JSImport.Namespace)
  private object QuotaModule extends js.Object {
    def newToken(resourceName: String, expectedUse: js.BigInt): JsQuotaTokenResource          = js.native
    def reserve(token: JsQuotaTokenResource, amount: js.BigInt): JsReservation                = js.native
    def split(token: JsQuotaTokenResource, childExpectedUse: js.BigInt): JsQuotaTokenResource = js.native
    def merge(token: JsQuotaTokenResource, other: JsQuotaTokenResource): Unit                 = js.native
  }

  @js.native
  @JSImport("golem:quota/types@1.5.0", "Reservation")
  private object JsReservationStatic extends js.Object {
    def commit(self: JsReservation, used: js.BigInt): Unit = js.native
  }
}
