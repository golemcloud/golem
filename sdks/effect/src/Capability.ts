/**
 * Opaque affine capabilities carried by schema values.
 *
 * Constructors require private SDK keys and therefore cannot forge host
 * resources. Encoding transfers ownership once, duplicate handles are rejected,
 * and composite conversion journals adopted resources for rollback on failure.
 * Live handles deliberately reject JSON serialization.
 *
 * @since 1.6.0
 */

/** @since 1.6.0 @category capabilities */
export type { GuestSecretHandle as Secret } from "./internal/schema-model/secretHandle.js"
/** @since 1.6.0 @category capabilities */
export type { GuestQuotaTokenHandle as QuotaToken } from "./internal/schema-model/quotaTokenHandle.js"
/** @since 1.6.0 @category capabilities */
export type { GuestPermissionCardHandle as PermissionCard } from "./internal/schema-model/permissionCardHandle.js"
