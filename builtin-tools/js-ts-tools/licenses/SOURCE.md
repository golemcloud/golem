# JavaScript toolchain source and notices

The embedded command implementations come from unmodified upstream package trees. Golem-owned wrappers apply version-guarded runtime compatibility patches and source-hash-guarded bundle transforms without modifying those trees.

- npm 10.9.9: https://github.com/npm/cli/tree/v10.9.9
  - Registry archive: https://registry.npmjs.org/npm/-/npm-10.9.9.tgz
  - Integrity: `sha512-1g+6jLQvaIuB4zwvHL7yrXuXcWZwDsCtBX8bbWDqbvJSSr9nPiDDWTHNgwXR27iIcTTW7v3A57hDW9RYv2W4Yg==`
  - Repacked artifact SHA-256: `c80bd097147cce32e31010b791bab8c71a26a5e10668210ec9d2e7f4c7cea2a1`
- TypeScript 5.9.2: https://github.com/microsoft/TypeScript/tree/v5.9.2
  - Registry archive: https://registry.npmjs.org/typescript/-/typescript-5.9.2.tgz
  - Integrity: `sha512-CWBzXQrc/qOkhidw1OzBTQuYRbfyxDXJMVJ1XNwUHGROVmuaeiEm3OslpZ1RV96d7SKKjZKrSJu3+t/xlw3R9A==`
  - Repacked artifact SHA-256: `67a3bc82e822b8f45f653a80fc3a9730d23214d36c83ba85dd7f5abebee82062`

The npm and TypeScript names are used descriptively. No upstream logos are used, no endorsement is implied, and the respective trademarks belong to their owners. Production publication requires legal and release-owner approval.
