# Supported versions (compatibility matrix)

This project targets a moving toolchain (Scala.js + golem-cli). The compatibility policy is:

- **runtime modules** (Scala.js/JVM libs) follow SemVer
- **tooling** (sbt/Mill plugins) follow SemVer, but may tighten requirements when `golem-cli` changes behavior

## Compatibility matrix (current repo state)

| Category | Supported |
|---|---|
| Scala | 2.13.x (runtime + test-agents + example), 3.8.2+ (runtime + tooling) |
| Scala.js | 1.20.x |
| sbt | 1.10+ (tested with 1.11.x) |
| Mill | 1.1.x (tested with 1.1.0-RC3; set `GOLEM_MILL_LIBS_VERSION` to compile the plugin against other 1.1.x versions) |
| golem-cli | 1.5.x (targets Golem v1.5 APIs) |

## Notes

- Tooling modules:
  - sbt plugin: `dev.zio:zio-golem-sbt` (provides `golem.sbt.GolemPlugin`)
  - Mill plugin: `dev.zio::zio-golem-mill` (provides `golem.mill.GolemAutoRegister`)
- If `golem-cli` changes invocation semantics again, update your invocation names accordingly (and then adjust any docs/scripts that call `golem`).
- The example/ and test-agents/ scripts default to `--local`; set `GOLEM_CLI_FLAGS="--cloud -p <profile>"` to target cloud.



