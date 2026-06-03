import { Search } from "nextra/components"

/**
 * Pagefind-backed search restricted to a single docs version.
 *
 * Indexed pages are tagged with `data-pagefind-filter="version:<slug>"`
 * by the version layout, so this filter scopes results to the active
 * version only. Without it, hits from every version of the docs would
 * be interleaved in the dropdown.
 */
export function VersionedSearch({ version }: { version: string }) {
  return <Search searchOptions={{ filters: { version } }} />
}
