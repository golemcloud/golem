import { notFound } from "next/navigation"
import Link from "next/link"
import { Layout, Navbar } from "nextra-theme-docs"
import { getPageMap } from "nextra/page-map"
import { GolemLogo } from "@/components/golem-logo"
import { Footer as GolemFooter } from "@/components/footer"
import { VersionSelector } from "@/components/version-selector"
import { VersionBanner } from "@/components/version-banner"
import { VersionedSearch } from "@/components/versioned-search"
import { DEFAULT_VERSION, VERSIONS, getVersion } from "@/lib/versions"
import { getVersionManifest } from "@/lib/version-manifest"

export function generateStaticParams() {
  return VERSIONS.map(v => ({ version: v.slug }))
}

export default async function VersionLayout({
  children,
  params,
}: {
  children: React.ReactNode
  params: Promise<{ version: string }>
}) {
  const { version } = await params
  const versionInfo = getVersion(version)
  if (!versionInfo) notFound()

  const [pageMap, manifest] = await Promise.all([getPageMap(`/${version}`), getVersionManifest()])

  return (
    <Layout
      navbar={
        <Navbar
          // Logo links to the active version's home so the selector and the
          // logo never bounce the user out of the current version. The
          // version selector itself is rendered as a child of <Navbar> (not
          // inside `logo`) because Nextra wraps the logo in an `<a>` element;
          // nesting the selector's clickable button inside that anchor (a) is
          // invalid HTML and (b) makes option clicks bubble up to the anchor
          // and navigate to `/` instead of switching versions.
          logo={
            <Link href={`/${version}`} aria-label="Home page" className="flex items-center">
              <GolemLogo />
            </Link>
          }
          logoLink={false}
          projectLink="https://github.com/golemcloud/golem"
          chatLink="https://discord.gg/UjXeH8uG4x"
        >
          <VersionSelector active={versionInfo} versions={[...VERSIONS]} manifest={manifest} />
        </Navbar>
      }
      footer={<GolemFooter />}
      docsRepositoryBase={`https://github.com/golemcloud/golem/blob/main/docs/src/content/${version}`}
      sidebar={{ defaultMenuCollapseLevel: 1, toggleButton: true }}
      pageMap={pageMap}
      nextThemes={{ defaultTheme: "dark" }}
      search={<VersionedSearch version={version} />}
    >
      {/* Tag indexed content with the active version so the Pagefind filter
          on <VersionedSearch> scopes search results to this version only. */}
      <div data-pagefind-filter={`version:${version}`} className="contents">
        <VersionBanner active={versionInfo} defaultVersion={DEFAULT_VERSION} />
        {children}
      </div>
    </Layout>
  )
}
