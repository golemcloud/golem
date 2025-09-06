## golem-ts-typegen

This package is designed to work with golem-ts-sdk, enabling the development of agents that run on Golem. Building a project that depends on golem-ts-sdk requires generating TypeScript types. This process is handled by golem-ts-typegen, which should always be included as a dev dependency.


### Example:

```sh

npx golem-typegen ./tsconfig.json --files tests/testData.ts

```
