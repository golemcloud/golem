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

/**
 * This is essentially what CLI is doing,
 * which is reading source file, creating metadata, serializing it to JSON
 * and load it to .metadata directory.
 * Every testing is performed on top of the metadata directory.
 */
import { Project } from 'ts-morph';
import { ClassMetadataGenConfig, generateClassMetadata } from '@golemcloud/golem-ts-typegen';

const project = new Project({
  tsConfigFilePath: 'tsconfig.json',
});

const sourceFiles = project.getSourceFiles('typegen-tests/testData.ts');

const genConfig: ClassMetadataGenConfig = {
  sourceFiles: sourceFiles,
  includeOnlyPublicScope: true,
  classDecorators: [],
  excludeOverriddenMethods: true,
};

generateClassMetadata(genConfig, project);
