#!/usr/bin/env node

import { Command } from 'commander';
import { Project } from 'ts-morph';
import pc from 'picocolors';
import logSymbols from 'log-symbols';
import { saveAndClearInMemoryMetadata, updateMetadataFromSourceFiles } from '../index.js';
import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { normalizeCliPath, normalizeFilePatterns } from './path-normalization.js';

const program = new Command();

program
  .name('golem-typegen')
  .description('Generate type metadata from TypeScript sources')
  .argument('<tsconfig>', 'Path to tsconfig.json')
  .option('-f, --files <patterns...>', 'File globs to include', ['src/**/*'])
  .option(
    '--include-class-decorators <names...>',
    'Only include classes decorated with these decorators (space separated)',
    [],
  )
  .option('--include-only-public-scope', 'Include only public scope methods and constructors', true)
  .option(
    '--exclude-overridden-methods',
    'Exclude methods that override parent class methods',
    true,
  )
  .option(
    '--golem-ts-sdk-import <value>',
    'Statement to use for importing the golem-ts-sdk',
    '@golemcloud/golem-ts-sdk',
  )
  .action(
    (
      tsconfig: string,
      options: {
        files: string[];
        includeClassDecorators: string[];
        includeOnlyPublicScope: boolean;
        excludeOverriddenMethods: boolean;
        golemTsSdkImport: string;
      },
    ) => {
      console.log(logSymbols.info, pc.cyan('Starting type metadata generation…'));

      const normalizedTsconfig = normalizeCliPath(tsconfig);
      const normalizedFilePatterns = normalizeFilePatterns(options.files);

      const project = new Project({ tsConfigFilePath: normalizedTsconfig });
      const sourceFiles = project.getSourceFiles(normalizedFilePatterns);

      console.log(logSymbols.info, pc.blue(`Processing ${sourceFiles.length} source files…`));

      const genConfig = {
        sourceFiles: sourceFiles,
        classDecorators: options.includeClassDecorators,
        includeOnlyPublicScope: options.includeOnlyPublicScope,
        excludeOverriddenMethods: options.excludeOverriddenMethods,
        golemTsSdkImport: options.golemTsSdkImport,
      };

      updateMetadataFromSourceFiles(genConfig, project);

      const result = TypeMetadata.getAll();

      if (result.size === 0) {
        console.warn(
          logSymbols.warning,
          pc.yellow('No agent classes extracted; metadata is empty.'),
        );
        console.warn(logSymbols.info, pc.gray(`tsconfig: ${normalizedTsconfig}`));
        console.warn(
          logSymbols.info,
          pc.gray(`file patterns: ${normalizedFilePatterns.join(', ')}`),
        );
      }

      console.log(
        logSymbols.success,
        pc.green(`Metadata tracked for: ${Array.from(result.keys()).join(', ')}`),
      );

      console.log(logSymbols.info, pc.yellow('Saving metadata…'));

      const filePath = saveAndClearInMemoryMetadata();

      console.log(logSymbols.success, pc.green(`Metadata saved successfully in ${filePath}!`));
    },
  );

program.parse();
