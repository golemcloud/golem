# Shared JavaScript SDK HTTP contract

This private source package owns the pure HTTP envelope, file mapping, router metadata,
header validation, and OpenAPI serialization primitives shared by the TypeScript and Effect SDKs.
It has no runtime dependencies or host imports. Its WIT type imports are checked against each
consuming SDK's generated declarations.

Both SDKs consume it as a development dependency and bundle its code and declarations into their
npm packages. It needs no separate installation, build, or publication. Run its contract tests
through `sdks/ts/packages/golem-ts-sdk/tests/http-router-contract.test.ts` and the SDK consumer tests.

After changing this source package, rerun `pnpm install` in `sdks/ts` to refresh pnpm's local
dependency copy before rebuilding the TypeScript SDK.
