import { Project } from "ts-morph";
//import { TypeMetadata } from "@golemcloud/golem-ts-sdk";

const project = new Project({
    tsConfigFilePath: "./tsconfig.json",
});
//
// const sourceFiles = project.getSourceFiles("src/**/*.ts");
//
// TypeMetadata.updateFromSourceFiles(sourceFiles)

// Import the user module after metadata is ready
// This needs to be done this way otherwise rollup ends up generating the module,
// where loading the metadata comes after the user module is loaded - resulting in errors.
export default (async () => {
    return await import("../src/main");
})();
