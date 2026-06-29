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

import { describe, expect, it } from 'vitest';
import { Project } from 'ts-morph';
import { getTypeFromTsMorph } from '../src/index.js';
import { createWellKnownTypes } from '../src/wellknownTypes.js';

describe('rich semantic author types', () => {
  it('maps author-facing types to rich type kinds', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './tests/sdkPlaceholder.ts');
    const sf = project.createSourceFile(
      '__rich_types_test__.ts',
      `
        import { Path, Duration, Quantity } from './tests/sdkPlaceholder.ts';
        type Meter = { baseUnit: 'm'; allowedSuffixes: ['m', 'cm'] };
        class Test {
          path!: Path;
          url!: URL;
          datetime!: Date;
          duration!: Duration;
          quantity!: Quantity<Meter>;
        }
      `,
      { overwrite: true },
    );
    const props = sf.getClassOrThrow('Test').getInstanceProperties();

    const types = props.map((p) => getTypeFromTsMorph(p.getType(), false, wellKnown));
    expect(types.map((type) => type.kind)).toEqual([
      'path',
      'url',
      'datetime',
      'duration',
      'quantity',
    ]);
    expect(types[4]).toMatchObject({
      kind: 'quantity',
      spec: { baseUnit: 'm', allowedSuffixes: ['m', 'cm'] },
    });
  });

  it('does not map user-defined Date and URL types to rich built-ins', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './tests/sdkPlaceholder.ts');
    const sf = project.createSourceFile(
      '__shadowed_rich_types_test__.ts',
      `
        export {};
        class Date { value!: string }
        interface URL { value: string }
        class Test {
          datetime!: Date;
          url!: URL;
        }
      `,
      { overwrite: true },
    );
    const props = sf.getClassOrThrow('Test').getInstanceProperties();

    const types = props.map((p) => getTypeFromTsMorph(p.getType(), false, wellKnown));
    expect(types.map((type) => type.kind)).toEqual(['class', 'interface']);
  });

  it('rejects Quantity specs without literal baseUnit and tuple allowedSuffixes', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './tests/sdkPlaceholder.ts');
    const sf = project.createSourceFile(
      '__invalid_quantity_test__.ts',
      `
        import { Quantity } from './tests/sdkPlaceholder.ts';
        type WidenedSuffixes = { baseUnit: 'm'; allowedSuffixes: string[] };
        type WidenedBaseUnit = { baseUnit: string; allowedSuffixes: ['m'] };
        class Test {
          widenedSuffixes!: Quantity<WidenedSuffixes>;
          widenedBaseUnit!: Quantity<WidenedBaseUnit>;
        }
      `,
      { overwrite: true },
    );
    const props = sf.getClassOrThrow('Test').getInstanceProperties();

    const types = props.map((p) => getTypeFromTsMorph(p.getType(), false, wellKnown));
    expect(types).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: 'unresolved-type',
          error: expect.stringContaining(
            'Quantity<T> type parameter must have a literal baseUnit and a tuple of string-literal allowedSuffixes',
          ),
        }),
      ]),
    );
    expect(types.every((type) => type.kind === 'unresolved-type')).toBe(true);
  });
});
