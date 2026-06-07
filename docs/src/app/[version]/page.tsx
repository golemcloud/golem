import { notFound } from "next/navigation"
import { importPage } from "nextra/pages"
import { useMDXComponents as getMDXComponents } from "../../../mdx-components"
import { VERSIONS, isValidVersion } from "@/lib/versions"

/**
 * Renders the index MDX for a version (e.g. /v1.5 -> v1.5/index.mdx).
 *
 * Split out from `[...mdxPath]/page.tsx` so that an optional catch-all
 * (`[[...mdxPath]]`) does not need to match the parent URL; an explicit
 * `page.tsx` here removes any ambiguity and keeps dev-mode routing happy.
 */
export function generateStaticParams() {
  return VERSIONS.map(v => ({ version: v.slug }))
}

export async function generateMetadata(props: { params: Promise<{ version: string }> }) {
  const params = await props.params
  if (!isValidVersion(params.version)) notFound()
  const { metadata } = await importPage([params.version])
  return metadata
}

const { wrapper: Wrapper } = getMDXComponents() as Record<string, React.FC<any>>

export default async function VersionIndexPage(props: { params: Promise<{ version: string }> }) {
  const params = await props.params
  if (!isValidVersion(params.version)) notFound()
  const { default: MDXContent, toc, metadata, sourceCode } = await importPage([params.version])
  return (
    <Wrapper toc={toc} metadata={metadata} sourceCode={sourceCode}>
      <MDXContent {...props} params={params} />
    </Wrapper>
  )
}
