import { redirect } from "next/navigation"
import { DEFAULT_VERSION } from "@/lib/versions"

export default function RootPage() {
  redirect(`/${DEFAULT_VERSION}`)
}
