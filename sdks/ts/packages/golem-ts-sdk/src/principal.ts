// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import {
  AgentPrincipal as HostAgentPrincipal,
  OidcPrincipal as HostOidcPrincipal,
  GolemUserPrincipal as HostGolemUserPrincipal,
  Principal as HostPrincipal,
} from 'golem:agent/common@1.5.0';

abstract class BasePrincipal {
  // prevents structural compatibility
  private readonly _brand!: void;

  abstract readonly tag: 'oidc' | 'agent' | 'golem-user' | 'anonymous';
}

class OidcPrincipal extends BasePrincipal {
  readonly tag = 'oidc' as const;

  constructor(readonly inner: HostOidcPrincipal) {
    super();
  }

  get sub() {
    return this.inner.sub;
  }
  get issuer() {
    return this.inner.issuer;
  }
  get email() {
    return this.inner.email;
  }
  get name() {
    return this.inner.name;
  }
  get emailVerified() {
    return this.inner.emailVerified;
  }
  get givenName() {
    return this.inner.givenName;
  }
  get familyName() {
    return this.inner.familyName;
  }
  get picture() {
    return this.inner.picture;
  }
  get preferredUsername() {
    return this.inner.preferredUsername;
  }
  get claims() {
    return this.inner.claims;
  }
}

class AgentPrincipal extends BasePrincipal {
  readonly tag = 'agent' as const;

  constructor(readonly inner: HostAgentPrincipal) {
    super();
  }

  get agentId() {
    return this.inner.agentId;
  }
}

class GolemUserPrincipal extends BasePrincipal {
  readonly tag = 'golem-user' as const;

  constructor(readonly inner: HostGolemUserPrincipal) {
    super();
  }

  get accountId() {
    return this.inner.accountId;
  }
}

class AnonymousPrincipal extends BasePrincipal {
  readonly tag = 'anonymous' as const;
}

export function sdkPrincipalFromHost(p: HostPrincipal): Principal {
  switch (p.tag) {
    case 'oidc':
      return new OidcPrincipal(p.val);
    case 'agent':
      return new AgentPrincipal(p.val);
    case 'golem-user':
      return new GolemUserPrincipal(p.val);
    case 'anonymous':
      return new AnonymousPrincipal();
  }
}

export function sdkPrincipalToHost(p: Principal): HostPrincipal {
  switch (p.tag) {
    case 'oidc':
      return { tag: 'oidc', val: p.inner };
    case 'agent':
      return { tag: 'agent', val: p.inner };
    case 'golem-user':
      return { tag: 'golem-user', val: p.inner };
    case 'anonymous':
      return { tag: 'anonymous' };
  }
}

export type Principal = OidcPrincipal | AgentPrincipal | GolemUserPrincipal | AnonymousPrincipal;
