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
import golem.data.GolemSchema
import golem.Uuid
import golem.EnvironmentId
import zio.blocks.schema.Schema

import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

/**
 * Scala.js facade for `golem:quota/types@1.5.0`.
 *
 * WIT interface:
 * {{{
 *   record failed-reservation { estimated-wait-nanos: option<u64> }
 *   record quota-token-record { environment-id, resource-name, expected-use, last-credit, last-credit-at }
 *   resource reservation { commit: func(used: u64) }
 *   resource quota-token {
 *     constructor(resource-name: string, expected-use: u64);
 *     reserve: func(amount: u64) -> result<reservation, failed-reservation>;
 *     split: func(child-expected-use: u64) -> quota-token;
 *     merge: func(other: quota-token);
 *     to-record: func() -> quota-token-record;
 *     from-record: static func(serialized: quota-token-record) -> quota-token;
 *   }
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
      underlying.commit(js.BigInt(used.toString))
  }

  // --- WIT: quota-token resource ---

  /**
   * An unforgeable capability granting the right to consume a named resource.
   *
   * Dropping the token releases the underlying lease back to the executor pool.
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
  final class QuotaToken private[golem] (private[golem] val underlying: JsQuotaToken) {

    /**
     * Reserve `amount` units from the local allocation.
     *
     * Blocks internally until capacity is available or the resource's
     * enforcement action fires. Returns `Right(reservation)` on success, or
     * `Left(FailedReservation)` when the enforcement policy is `reject`. For
     * `throttle` / `terminate` policies the call suspends or terminates the
     * agent before returning.
     */
    def reserve(amount: BigInt): Either[FailedReservation, Reservation] =
      try {
        val raw = underlying.reserve(js.BigInt(amount.toString))
        Right(new Reservation(raw))
      } catch {
        case e: js.JavaScriptException =>
          val raw           = e.exception.asInstanceOf[JsFailedReservation]
          val estimatedWait = raw.estimatedWaitNanos.toOption.map(bi => BigInt(bi.toString))
          Left(FailedReservation(estimatedWait))
      }

    /**
     * Split off a child token with `childExpectedUse` units of expected-use.
     *
     *   - The parent's expected-use is reduced by `childExpectedUse`.
     *   - Credits are divided proportionally between parent and child.
     *   - Both tokens share the same underlying lease.
     *
     * Traps if `childExpectedUse` exceeds the parent's current expected-use.
     */
    def split(childExpectedUse: BigInt): QuotaToken =
      new QuotaToken(underlying.split(js.BigInt(childExpectedUse.toString)))

    /**
     * Merge `other` back into this token.
     *
     * Combines expected-use and credits. `other` is consumed by this call and
     * must not be used afterwards.
     *
     * Traps if the tokens refer to different resources.
     */
    def merge(other: QuotaToken): Unit =
      underlying.merge(other.underlying)

    private[golem] def toRecord(): QuotaTokenRecord = {
      val raw = underlying.toRecord()
      val ts  = raw.lastCreditAt
      QuotaTokenRecord(
        environmentId = environmentIdFromJs(raw.environmentId.asInstanceOf[JsEnvironmentId]),
        resourceName = raw.resourceName,
        expectedUse = BigInt(raw.expectedUse.toString),
        lastCredit = BigInt(raw.lastCredit.toString),
        lastCreditAtSeconds = BigInt(ts.seconds.toString),
        lastCreditAtNanos = ts.nanoseconds
      )
    }
  }

  /** A serializable snapshot of a [[QuotaToken]], suitable for RPC transfer. */
  final case class QuotaTokenRecord(
    environmentId: EnvironmentId,
    resourceName: String,
    expectedUse: BigInt,
    lastCredit: BigInt,
    lastCreditAtSeconds: BigInt,
    lastCreditAtNanos: Int
  )

  object QuotaTokenRecord {
    implicit val schema: Schema[QuotaTokenRecord] = Schema.derived
  }

  object QuotaToken {

    private[golem] def fromRecord(record: QuotaTokenRecord): QuotaToken = {
      val jsRecord = js.Dynamic
        .literal(
          environmentId = environmentIdToJs(record.environmentId),
          resourceName = record.resourceName,
          expectedUse = js.BigInt(record.expectedUse.toString),
          lastCredit = js.BigInt(record.lastCredit.toString),
          lastCreditAt = JsDatetime(
            js.BigInt(record.lastCreditAtSeconds.toString),
            record.lastCreditAtNanos
          )
        )
        .asInstanceOf[JsQuotaTokenRecord]
      new QuotaToken(JsQuotaTokenStaticClass.fromRecord(jsRecord).asInstanceOf[JsQuotaToken])
    }

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
        new JsQuotaTokenClass(resourceName, js.BigInt(expectedUse.toString))
          .asInstanceOf[JsQuotaToken]
      )

    /**
     * Automatic serialization for RPC: `QuotaToken` is transparently converted
     * to/from [[QuotaTokenRecord]] when passing across agent boundaries.
     *
     * Users do not need to call `toRecord` / `fromRecord` manually.
     */
    implicit val golemSchema: GolemSchema[QuotaToken] = {
      val recordSchema = GolemSchema.fromBlocksSchema[QuotaTokenRecord]
      new GolemSchema[QuotaToken] {
        override def schema                                        = recordSchema.schema
        override def elementSchema                                 = recordSchema.elementSchema
        override def encode(value: QuotaToken)                     = recordSchema.encode(value.toRecord())
        override def decode(value: golem.data.StructuredValue)     = recordSchema.decode(value).map(fromRecord)
        override def encodeElement(value: QuotaToken)              = recordSchema.encodeElement(value.toRecord())
        override def decodeElement(value: golem.data.ElementValue) = recordSchema.decodeElement(value).map(fromRecord)
      }
    }
  }

  @js.native
  @JSImport("golem:quota/types@1.5.0", "QuotaToken")
  private class JsQuotaTokenClass(resourceName: String, expectedUse: js.BigInt) extends js.Object {
    def reserve(amount: js.BigInt): JsReservation        = js.native
    def split(childExpectedUse: js.BigInt): JsQuotaToken = js.native
    def merge(other: JsQuotaToken): Unit                 = js.native
    def toRecord(): JsQuotaTokenRecord                   = js.native
  }

  @js.native
  @JSImport("golem:quota/types@1.5.0", "QuotaToken")
  private object JsQuotaTokenStaticClass extends js.Object {
    def fromRecord(serialized: JsQuotaTokenRecord): js.Object = js.native
  }

  private def environmentIdToJs(environmentId: EnvironmentId): JsEnvironmentId =
    JsEnvironmentId(
      uuid = JsUuid(
        highBits = js.BigInt(environmentId.uuid.highBits.toString),
        lowBits = js.BigInt(environmentId.uuid.lowBits.toString)
      )
    )

  private def environmentIdFromJs(raw: JsEnvironmentId): EnvironmentId =
    EnvironmentId(
      uuid = Uuid(
        highBits = BigInt(raw.uuid.highBits.toString),
        lowBits = BigInt(raw.uuid.lowBits.toString)
      )
    )
}
