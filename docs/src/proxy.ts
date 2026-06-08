import { NextResponse, type NextRequest } from "next/server"
import { DEFAULT_VERSION, VERSION_SLUGS } from "./lib/versions"

/**
 * Redirect legacy un-versioned URLs (e.g., `/quickstart` from old inbound
 * links or search-engine results) to the default version's equivalent
 * (`/v1.5/quickstart`). Requests that already start with a known version
 * slug are passed through unchanged.
 *
 * Static assets, Next.js internals, the search index, and anything with a
 * file extension are excluded via the `matcher` below.
 */
export function proxy(req: NextRequest) {
  const { pathname } = req.nextUrl
  const first = pathname.split("/")[1] ?? ""

  if (VERSION_SLUGS.includes(first)) return NextResponse.next()

  const url = req.nextUrl.clone()
  url.pathname = `/${DEFAULT_VERSION}${pathname === "/" ? "" : pathname}`
  return NextResponse.redirect(url, 308)
}

export const config = {
  matcher: [
    // Match everything except Next internals, Pagefind output, common static
    // files, and any path containing a "." (i.e., file extensions). The
    // remaining unversioned doc paths are redirected to DEFAULT_VERSION.
    "/((?!_next/|_pagefind/|favicon\\.ico|robots\\.txt|sitemap\\.xml|.*\\..*).*)",
  ],
}
