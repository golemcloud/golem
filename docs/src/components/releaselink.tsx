import { RELEASE, Artifact, Platform } from "@/lib/releases"
import Link from "next/link"

export function ReleaseLink({
  children = "release page",
  version = RELEASE.version,
}: {
  children?: React.ReactNode
  version?: string
}) {
  const url = `${RELEASE.baseReleaseUrl}/${version}`
  return <Link href={url}>{children}</Link>
}

export function ArtifactLink({
  artifact,
  platform,
  version = RELEASE.version,
}: {
  artifact: Artifact
  platform: Platform
  version?: string
}) {
  const file = RELEASE.artifacts[artifact][platform]
  const url = `${RELEASE.baseDownloadUrl}/${version}/${file}`

  return <Link href={url}>{file}</Link>
}
