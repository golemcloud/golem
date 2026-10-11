# Git tool source and notices

The Git object and worktree implementation is provided by pinned upstream packages. Golem-owned wrappers expose local and HTTP(S) workflows and integrate them with the component filesystem. The isomorphic-git source changes are preserved in the source repository's checksum-pinned transfer patch.

- isomorphic-git 1.42.2: https://github.com/isomorphic-git/isomorphic-git/tree/v1.42.2
  - Registry archive: https://registry.npmjs.org/isomorphic-git/-/isomorphic-git-1.42.2.tgz
  - Integrity: `sha512-Rb7czWhLO4JYqmg4an01nNomuAgFbWWK2CVfkHGBOvK5t7hrGTKOXPx/csscJ1j9o3uXFzNcC6njgvCafSmIPw==`
  - Repacked artifact SHA-256: `9b01a8490da9a95175e595653d69a159de54e09d2814e2ea5c867c8dc12983b0`
- diff 8.0.2: https://github.com/kpdecker/jsdiff/tree/v8.0.2
  - Registry archive: https://registry.npmjs.org/diff/-/diff-8.0.2.tgz
  - Integrity: `sha512-sSuxWU5j5SR9QQji/o2qMvqRNYRDOcBTgsJ/DeCf4iSN4gW+gNMXM7wFIP+fdXZxoNiAnHUTGjCr+TSWXdRDKg==`
  - Repacked artifact SHA-256: `7bdbefb2df0f28dc9280e39e2399f2e6e84d13e9ccb098469b32ad0372c0f5e3`
- zod 4.4.3: https://github.com/colinhacks/zod/tree/v4.4.3
  - Registry archive: https://registry.npmjs.org/zod/-/zod-4.4.3.tgz
  - Integrity: `sha512-ytENFjIJFl2UwYglde2jchW2Hwm4GJFLDiSXWdTrJQBIN9Fcyp7n4DhxJEiWNAJMV1/BqWfW/kkg71UDcHJyTQ==`
  - Repacked artifact SHA-256: `ee38f17f533fd500610685a483ae2f413c26f4eb33a51684314563c8d60f279c`

- Git xdiff 2.55.0: https://github.com/git/git/tree/v2.55.0/xdiff
  - Source archive: https://www.kernel.org/pub/software/scm/git/git-2.55.0.tar.xz
  - Source SHA-256: `457fdb04dc8728e007d4688695e6912e6f680727920f2a40bf11eacc17505357`
  - License: LGPL-2.1-or-later AND EDL-1.0
  - Preferred source, original notices, build glue and rebuilding instructions: `git/xdiff/` in the license bundle.

The Git and upstream package names are used descriptively. No upstream logos are used, no endorsement is implied, and the respective trademarks belong to their owners. Production publication requires legal and release-owner approval, including review of the xdiff library's LGPL distribution obligations.
