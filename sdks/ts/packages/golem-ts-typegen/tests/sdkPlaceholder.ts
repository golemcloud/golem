abstract class BasePrincipal {
  private readonly _brand!: void;

  abstract readonly tag: 'oidc' | 'agent' | 'golem-user' | 'anonymous';
}

export class OidcPrincipal extends BasePrincipal {
  readonly tag = 'oidc' as const;
}

export class AgentPrincipal extends BasePrincipal {
  readonly tag = 'agent' as const;
}

export class GolemUserPrincipal extends BasePrincipal {
  readonly tag = 'golem-user' as const;
}

export class AnonymousPrincipal extends BasePrincipal {
  readonly tag = 'anonymous' as const;
}

export type Principal = OidcPrincipal | AgentPrincipal | GolemUserPrincipal | AnonymousPrincipal;

export class Secret<T> {}

export class Config<T> {}

export class QuotaToken {}
