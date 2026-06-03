/**
 * Central registry of documentation versions.
 *
 * URL structure is `/{slug}/...`. The root `/` redirects to `DEFAULT_VERSION`
 * (see `src/app/page.tsx` and `src/middleware.ts`).
 *
 * To cut a new release:
 *   1. Promote `next` -> the new version slug with the version tool:
 *        bun run scripts/version-tool.ts rename next v1.6
 *   2. Re-clone `next` from the new current version:
 *        bun run scripts/version-tool.ts clone v1.6 next
 *   3. Update this file: add the new version as `current`, demote the previous
 *      `current` to `legacy`, and bump `DEFAULT_VERSION`.
 */

export type VersionStatus = "current" | "unreleased" | "legacy"

export type Version = {
  /** URL slug — also the on-disk directory name under `src/content/`. */
  slug: string
  /** Short label shown in the version selector. */
  label: string
  /** Lifecycle status of the version. */
  status: VersionStatus
}

export const VERSIONS: readonly Version[] = [
  { slug: "v1.5", label: "v1.5", status: "current" },
  { slug: "next", label: "Next", status: "unreleased" },
] as const

export const DEFAULT_VERSION: string = "v1.5"

export const VERSION_SLUGS: readonly string[] = VERSIONS.map(v => v.slug)

export function getVersion(slug: string | undefined): Version | undefined {
  return VERSIONS.find(v => v.slug === slug)
}

export function isValidVersion(slug: string | undefined): slug is string {
  return slug !== undefined && VERSION_SLUGS.includes(slug)
}

export function getCurrentVersion(): Version {
  const current = VERSIONS.find(v => v.status === "current")
  if (!current) throw new Error("No version is marked as 'current' in VERSIONS")
  return current
}

export function statusBadge(status: VersionStatus): string {
  switch (status) {
    case "current":
      return "current"
    case "unreleased":
      return "unreleased"
    case "legacy":
      return "legacy"
  }
}
