"use client"

// Silences a single, harmless React 19 dev-mode warning emitted by
// `next-themes` (used by Nextra). `next-themes` v0.4.6 renders an inline
// `<script>` to set the theme before hydration (preventing FOUC); React 19's
// dev mode warns about any `<script>` rendered as a React element. The script
// actually does run correctly during SSR — the warning is a false positive
// and the upstream package has been unmaintained for over a year.
//
// Refs:
//   - https://github.com/pacocoursey/next-themes/issues/385
//   - https://github.com/shadcn-ui/ui/issues/10104
if (typeof window !== "undefined" && process.env.NODE_ENV === "development") {
  const orig = console.error
  console.error = (...args: unknown[]) => {
    const first = args[0]
    if (typeof first === "string" && first.includes("Encountered a script tag")) return
    orig.apply(console, args)
  }
}

export function DevWarningFilter() {
  return null
}
