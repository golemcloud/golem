#### Build and Test

```sh
npx pnpm clean # if needed
npx pnpm install
npx pnpm run format
npx pnpm run format:check
npx pnpm run lint
npx pnpm run build
npx pnpm run test # or per package: Ex: cd packages/golem-ts-sdk && pnpm run test
```

If making changes to `golem-ts-typegen` or `golem-ts-types-core`, it's good to run `pnpm install` and `pnpm run build` (from root) before
running tests in `golem-ts-sdk`, to make sure it uses the latest installed `golem-typegen`.

`pnpm run test` within packages will run tests without forgetting console logs, and will be more faster.
