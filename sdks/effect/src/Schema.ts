/**
 * Effect Schema facade with Golem's WIT-specific schemas and annotations.
 *
 * Authoring uses Effect Schema, including its transformations, services, and typed failures.
 * Standard Schema walkers and third-party schema adapters are not part of this package.
 * Cross-language interoperability uses canonical Golem schema graphs and value trees, not
 * a shared JavaScript schema library; generated clients can use the same wire metadata.
 *
 * @since 1.6.0
 */

/** @since 1.6.0 @category schemas */
export * from "effect/Schema"
/** @since 1.6.0 @category schemas */
export {
  AgentStream,
  Binary,
  Datetime,
  DiscriminatedUnion,
  FixedList,
  Flags,
  Map,
  Path,
  PermissionCard,
  Quantity,
  QuotaToken,
  Secret,
  Text,
  Url,
  metadata,
  Duration as NanosecondDuration,
  Char as WitChar,
} from "./WitTypes.js"
