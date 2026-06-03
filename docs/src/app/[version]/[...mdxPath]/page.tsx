import { notFound } from "next/navigation"
import { generateStaticParamsFor, importPage } from "nextra/pages"
import { useMDXComponents as getMDXComponents } from "../../../../mdx-components"
import { isValidVersion } from "@/lib/versions"

const generateAllParams = generateStaticParamsFor("mdxPath")

export async function generateStaticParams() {
  const all = (await generateAllParams()) as { mdxPath: string[] }[]
  // Nextra returns the full set of MDX paths under `src/content/`. Because all
  // content lives in versioned subdirectories (e.g. `v1.5/quickstart`), the
  // first segment of each entry is the version slug; split it off so it maps
  // onto our `[version]/[...mdxPath]` route. The version index pages (i.e.,
  // entries with a single segment like just `["v1.5"]`) are NOT served by
  // this route — they're handled by `src/app/[version]/page.tsx`, so we
  // require at least one sub-path segment here.
  return all
    .filter(p => p.mdxPath?.length > 1 && isValidVersion(p.mdxPath[0]))
    .map(p => ({
      version: p.mdxPath[0],
      mdxPath: p.mdxPath.slice(1),
    }))
}

export async function generateMetadata(props: {
  params: Promise<{ version: string; mdxPath: string[] }>
}) {
  const params = await props.params
  if (!isValidVersion(params.version)) notFound()
  const { metadata } = await importPage([params.version, ...params.mdxPath])
  return metadata
}

const { wrapper: Wrapper } = getMDXComponents() as Record<string, React.FC<any>>

export default async function Page(props: {
  params: Promise<{ version: string; mdxPath: string[] }>
}) {
  const params = await props.params
  if (!isValidVersion(params.version)) notFound()
  const {
    default: MDXContent,
    toc,
    metadata,
    sourceCode,
  } = await importPage([params.version, ...params.mdxPath])
  return (
    <Wrapper toc={toc} metadata={metadata} sourceCode={sourceCode}>
      <MDXContent {...props} params={params} />
    </Wrapper>
  )
}
