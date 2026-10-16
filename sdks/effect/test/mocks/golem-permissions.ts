/** Tests using permissions supply a PermissionsClient layer with their own card store. */
function unavailable(): never {
  throw new Error("provide a PermissionsClient test layer")
}
export const id = unavailable
export const parents = unavailable
export const expiresAt = unavailable
export const inspectCard = unavailable
export const derive = unavailable
export const deriveFromWallet = unavailable
export const deriveScope = unavailable
export const revokeCard = unavailable
export const selfWallet = unavailable
export const selfVersion = unavailable
export const installCard = unavailable
export const listModules = unavailable
export const validateGrant = unavailable
