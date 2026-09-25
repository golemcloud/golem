import type { RollupOptions, rollup } from "rollup"

/** Generate static guest exports from a tree-shaken component. @since 1.6.0 @category build */
export declare function componentConfiguration(
  build: typeof rollup,
  options: RollupOptions,
): Promise<RollupOptions>
