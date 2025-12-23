import { RELEASE, Artifact, Platform } from "@/lib/releases"
import { Link } from "nextra-theme-docs"

export function ReleaseLink({ children = "release page" }: { children?: React.ReactNode }) {
  const url = `${RELEASE.baseReleaseUrl}/${RELEASE.version}`
  return <Link href={url}>{children}</Link>
}

export function ArtifactLink({ artifact, platform }: { artifact: Artifact; platform: Platform }) {
  const file = RELEASE.artifacts[artifact][platform]
  const url = `${RELEASE.baseDownloadUrl}/${RELEASE.version}/${file}`

  return <Link href={url}>{file}</Link>
}
