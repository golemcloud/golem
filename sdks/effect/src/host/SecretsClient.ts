import { Context, Layer } from "effect"
import type * as CoreTypes from "golem:core/types@2.0.0"
import * as RevealHost from "golem:secrets/reveal@0.1.0"
import * as SecretsHost from "golem:secrets/types@0.1.0"

export interface SecretsClientShape {
  readonly reveal: (
    secret: CoreTypes.Secret,
    expected: CoreTypes.SchemaGraph,
  ) => CoreTypes.SchemaValueTree
  readonly id: (secret: CoreTypes.Secret) => SecretsHost.SecretId
  readonly metadata: (secret: CoreTypes.Secret) => SecretsHost.SecretMetadata
}

export class SecretsClient extends Context.Service<SecretsClient, SecretsClientShape>()(
  "effect-golem/host/Secrets",
) {}

export const SecretsLive: Layer.Layer<SecretsClient> = Layer.succeed(
  SecretsClient,
  SecretsClient.of({
    reveal: RevealHost.reveal,
    id: SecretsHost.id,
    metadata: SecretsHost.metadata,
  }),
)
