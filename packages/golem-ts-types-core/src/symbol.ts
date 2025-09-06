// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import { Type } from './type-lite';
import { Node } from './node';

export class Symbol {
  private readonly name: string;
  private readonly decls: Node[];
  private readonly valueDecl?: Node;
  private readonly _typeAtLocation: Type;
  private readonly _aliasTarget?: Type;

  constructor(args: {
    name: string;
    declarations: Node[];
    typeAtLocation?: Type;
    valueDeclaration?: Node;
    aliasTarget?: Type;
  }) {
    this.name = args.name;
    this.decls = args.declarations;
    this.valueDecl = args.valueDeclaration ?? args.declarations[0];
    this._typeAtLocation = args.typeAtLocation ?? {
      kind: 'undefined',
      name: 'undefined',
    };
    this._aliasTarget = args.aliasTarget;
  }

  getName(): string {
    return this.name;
  }

  getDeclarations(): Node[] {
    return this.decls;
  }

  getValueDeclarationOrThrow(): Node {
    if (!this.valueDecl) throw new Error('No value declaration');
    return this.valueDecl;
  }

  getTypeAtLocation(_node: Node): Type {
    return this._typeAtLocation;
  }

  getAliasTarget(): Type | undefined {
    return this._aliasTarget;
  }
}
