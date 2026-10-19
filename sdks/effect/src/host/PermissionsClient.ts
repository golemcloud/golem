import { Context, Layer } from "effect"
import type { PermissionCard } from "golem:core/types@2.0.0"
import * as DeriveHost from "golem:permissions/derive@0.1.0"
import * as InspectHost from "golem:permissions/inspect@0.1.0"
import * as KernelHost from "golem:permissions/kernel-introspection@0.1.0"
import * as RevokeHost from "golem:permissions/revoke@0.1.0"
import * as TypesHost from "golem:permissions/types@0.1.0"
import * as WalletHost from "golem:permissions/wallet@0.1.0"

type Grants = TypesHost.PatternGrant[]

export interface PermissionsClientShape {
  readonly id: (card: PermissionCard) => TypesHost.CardId
  readonly parents: (card: PermissionCard) => TypesHost.CardId[]
  readonly expiresAt: (card: PermissionCard) => TypesHost.Timestamp | undefined
  readonly inspect: (card: PermissionCard) => TypesHost.CardView
  readonly derive: (
    parent: PermissionCard,
    lowerPositiveToRetain: Grants,
    lowerNegativeToAdd: Grants,
    upperPositiveToRetain: Grants,
    upperNegativeToAdd: Grants,
    lifespan: TypesHost.Duration | undefined,
  ) => TypesHost.PersistentCard
  readonly deriveFromWallet: (
    lowerPositive: Grants,
    lowerNegative: Grants,
    upperPositive: Grants,
    upperNegative: Grants,
    lifespan: TypesHost.Duration | undefined,
  ) => TypesHost.PersistentCard
  readonly deriveScope: (
    parents: PermissionCard[],
    lowerPositive: Grants,
    lowerNegative: Grants,
    upperPositive: Grants,
    upperNegative: Grants,
  ) => PermissionCard
  readonly revoke: (card: PermissionCard) => number
  readonly wallet: () => TypesHost.WalletCard[]
  readonly walletVersion: () => TypesHost.WalletVersionToken
  readonly install: (card: PermissionCard, target: TypesHost.Holder) => void
  readonly listModules: () => KernelHost.KernelModule[]
  readonly validateGrant: (grant: TypesHost.PatternGrant) => void
  readonly drop: (card: PermissionCard) => void
}

export class PermissionsClient extends Context.Service<PermissionsClient, PermissionsClientShape>()(
  "effect-golem/host/Permissions",
) {}

export const PermissionsLive: Layer.Layer<PermissionsClient> = Layer.succeed(
  PermissionsClient,
  PermissionsClient.of({
    id: TypesHost.id,
    parents: TypesHost.parents,
    expiresAt: TypesHost.expiresAt,
    inspect: InspectHost.inspectCard,
    derive: DeriveHost.derive,
    deriveFromWallet: DeriveHost.deriveFromWallet,
    deriveScope: DeriveHost.deriveScope,
    revoke: RevokeHost.revokeCard,
    wallet: WalletHost.selfWallet,
    walletVersion: WalletHost.selfVersion,
    install: WalletHost.installCard,
    listModules: KernelHost.listModules,
    validateGrant: KernelHost.validateGrant,
    drop: (card) => {
      ;(card as { [Symbol.dispose]?: () => void })[Symbol.dispose]?.()
    },
  }),
)
