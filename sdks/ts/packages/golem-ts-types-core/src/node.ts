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

type NodeKind = 'PropertySignature' | 'PropertyDeclaration' | 'TypeAlias';

export class Node {
  private readonly _kind: NodeKind;
  private readonly _optional: boolean;

  constructor(kind: NodeKind, optional = false) {
    this._kind = kind;
    this._optional = optional;
  }

  getText(): string {
    return this._kind;
  }

  hasQuestionToken(): boolean {
    return this._optional;
  }

  static isPropertySignature(node: Node): boolean {
    return node._kind === 'PropertySignature';
  }

  static isPropertyDeclaration(node: Node): boolean {
    return node._kind === 'PropertyDeclaration';
  }

  _getKind(): NodeKind {
    return this._kind;
  }
}
