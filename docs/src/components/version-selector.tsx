"use client"

import { useEffect, useRef, useState } from "react"
import { usePathname, useRouter } from "next/navigation"
import { ChevronDownIcon, CheckIcon } from "@radix-ui/react-icons"
import { statusBadge, type Version } from "@/lib/versions"
import type { VersionManifest } from "@/lib/version-manifest"

type Props = {
  active: Version
  versions: Version[]
  manifest: VersionManifest
}

/**
 * Compact dropdown that switches the currently-displayed docs version.
 *
 * Path mapping on switch:
 *   1. Try the same sub-path under the target version.
 *   2. Otherwise walk up parent paths until one exists.
 *   3. Otherwise fall back to the target version's index page.
 */
export function VersionSelector({ active, versions, manifest }: Props) {
  const router = useRouter()
  const pathname = usePathname() ?? `/${active.slug}`
  const [open, setOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)

  // Close on outside click / Escape.
  useEffect(() => {
    if (!open) return
    const onDocClick = (e: MouseEvent) => {
      if (!containerRef.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false)
    }
    document.addEventListener("mousedown", onDocClick)
    document.addEventListener("keydown", onKey)
    return () => {
      document.removeEventListener("mousedown", onDocClick)
      document.removeEventListener("keydown", onKey)
    }
  }, [open])

  const onSelect = (target: Version) => {
    setOpen(false)
    if (target.slug === active.slug) return

    const segments = pathname.split("/").filter(Boolean)
    const rest = segments.slice(1) // strip current version slug

    const targetRoutes = new Set(manifest[target.slug] ?? [])
    let candidate = rest.join("/")
    while (candidate.length > 0 && !targetRoutes.has(candidate)) {
      const slash = candidate.lastIndexOf("/")
      candidate = slash === -1 ? "" : candidate.slice(0, slash)
    }
    const href = candidate ? `/${target.slug}/${candidate}` : `/${target.slug}`
    router.push(href)
  }

  return (
    <div ref={containerRef} className="relative">
      <button
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen(o => !o)}
        className="inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs font-medium text-gray-700 transition-colors hover:bg-gray-100 dark:border-neutral-700 dark:text-gray-200 dark:hover:bg-neutral-800"
      >
        <span>{active.label}</span>
        <span className="rounded bg-gray-200 px-1.5 py-px text-[10px] uppercase tracking-wide text-gray-700 dark:bg-neutral-700 dark:text-gray-200">
          {statusBadge(active.status)}
        </span>
        <ChevronDownIcon
          className={`h-3.5 w-3.5 transition-transform ${open ? "rotate-180" : ""}`}
        />
      </button>

      {open && (
        <ul
          role="listbox"
          className="absolute left-0 z-50 mt-1 min-w-[14rem] overflow-hidden rounded-md border bg-white py-1 text-sm shadow-lg dark:border-neutral-700 dark:bg-neutral-900"
        >
          {versions.map(v => {
            const isActive = v.slug === active.slug
            return (
              <li key={v.slug}>
                <button
                  type="button"
                  role="option"
                  aria-selected={isActive}
                  onClick={() => onSelect(v)}
                  className={`flex w-full items-center justify-between gap-3 px-3 py-1.5 text-left transition-colors hover:bg-gray-100 dark:hover:bg-neutral-800 ${
                    isActive ? "font-semibold" : ""
                  }`}
                >
                  <span className="flex items-center gap-2">
                    <span>{v.label}</span>
                    <span className="rounded bg-gray-200 px-1.5 py-px text-[10px] uppercase tracking-wide text-gray-700 dark:bg-neutral-700 dark:text-gray-200">
                      {statusBadge(v.status)}
                    </span>
                  </span>
                  {isActive && <CheckIcon className="h-3.5 w-3.5" />}
                </button>
              </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}
