# Git xdiff source and rebuilding

This bundle preserves the preferred C source of the Git 2.55.0 xdiff library, including each original copyright/license header, the LGPL-2.1 license and its referenced GPL text. The histogram implementation includes its full EDL-1.0 notice in source/xhistogram.c. Golem's compatibility header and normalized-diff.c are Apache-2.0 licensed. Upstream xdiff source is unmodified; these two glue files supply an import-free bounded allocator and normalized output callback.

The component executes transpiled JavaScript, not a second WASM module or native Git subprocess. build-xdiff.mjs and provenance.json preserve the exact toolchain and transformation.

To rebuild or replace this library in the tool, obtain the golem source commit recorded in the release provenance, install WASI SDK 25 (clang 19.1.5-wasi-sdk), set WASI_SDK_PATH to its directory, and run npm ci --ignore-scripts in builtin-tools/git. Run npm run build:git-library, then the focused cargo make build-git-tool task from the repository root. The latter bundles the regenerated library and rebuilds the component through its Golem manifest. Generated output lives in golem-temp/xdiff and is not committed.

For a modified xdiff library, modify the checksum-verified extracted C source and/or the two glue files, adapt build-xdiff.mjs to use that modified source, and update the expected generatedJavaScriptSha256 after inspecting the result. The source archive, compiler and generated-output checks are reproducibility checks, not a prohibition on replacing the library. Rebuild the tool as above to relink it. Preserve modified source and its applicable notices alongside any redistributed component.
