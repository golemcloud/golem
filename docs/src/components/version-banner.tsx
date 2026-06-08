import Link from "next/link"
import type { Version } from "@/lib/versions"

type Props = {
  active: Version
  defaultVersion: string
}

/**
 * Banner shown above the page body whenever the user is viewing docs for a
 * non-current version (unreleased preview or legacy release). Hidden on the
 * current stable version.
 */
export function VersionBanner({ active, defaultVersion }: Props) {
  if (active.status === "current") return null

  const isUnreleased = active.status === "unreleased"
  const bg = isUnreleased
    ? "bg-amber-100 dark:bg-amber-900/30 border-amber-300 dark:border-amber-700/60 text-amber-900 dark:text-amber-100"
    : "bg-blue-100 dark:bg-blue-900/30 border-blue-300 dark:border-blue-700/60 text-blue-900 dark:text-blue-100"

  const message = isUnreleased
    ? "You're reading the unreleased preview docs. APIs and pages may change before the next release."
    : "You're reading docs for a previous release."

  return (
    <div
      className={`my-4 flex flex-col gap-1 rounded-md border px-4 py-3 text-sm sm:flex-row sm:items-center sm:justify-between ${bg}`}
    >
      <span>
        <strong className="font-semibold">{active.label}</strong> — {message}
      </span>
      <Link
        href={`/${defaultVersion}`}
        className="font-medium underline underline-offset-2 hover:no-underline"
      >
        Switch to the current docs →
      </Link>
    </div>
  )
}
