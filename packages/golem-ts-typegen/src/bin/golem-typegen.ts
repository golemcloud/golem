#!/usr/bin/env node

import { Command } from "commander";
import { Project } from "ts-morph";
import chalk from "chalk";
import logSymbols from "log-symbols";
import {
  saveAndClearInMemoryMetadata,
  updateMetadataFromSourceFiles,
} from "../index.js";
import { TypeMetadata } from "@golemcloud/golem-ts-types-core";
import path from "path";

const program = new Command();

program
  .name("golem-typegen")
  .description("Generate type metadata from TypeScript sources")
  .argument("<tsconfig>", "Path to tsconfig.json")
  .option("-f, --files <patterns...>", "File globs to include", ["src/**/*"])
  .action((tsconfig: string, options: { files: string[] }) => {
    console.log(
      logSymbols.info,
      chalk.cyan("Starting type metadata generation…"),
    );

    const project = new Project({ tsConfigFilePath: path.resolve(tsconfig) });
    const sourceFiles = project.getSourceFiles(options.files);

    console.log(
      logSymbols.info,
      chalk.blue(`Processing ${sourceFiles.length} source files…`),
    );

    updateMetadataFromSourceFiles(sourceFiles);

    const result = TypeMetadata.getAll();

    console.log(
      logSymbols.success,
      chalk.green(
        `Metadata tracked for: ${Array.from(result.keys()).join(", ")}`,
      ),
    );

    console.log(logSymbols.info, chalk.yellow("Saving metadata…"));
    const filePath = saveAndClearInMemoryMetadata();

    console.log(
      logSymbols.success,
      chalk.green(`Metadata saved successfully in ${filePath}!`),
    );
  });

program.parse();
