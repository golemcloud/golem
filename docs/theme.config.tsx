import React from "react"
import { DocsThemeConfig } from "nextra-theme-docs"
import { Footer } from "@/components/footer"
import { GolemLogo } from "@/components/golem-logo"
import { useRouter } from "next/router"

const config: DocsThemeConfig = {
  logo: <GolemLogo />,
  banner: {
    key: "docs-launch",
    text: (
      <div className="flex justify-center items-center gap-2">
        Welcome to the new Golem Cloud Docs! 👋
      </div>
    ),
  },
  primaryHue: {
    dark: 213,
    light: 226,
  },
  primarySaturation: {
    light: 90,
    dark: 93,
  },
  sidebar: {
    toggleButton: true,
  },
  project: {
    link: "https://github.com/golemcloud/docs",
  },
  chat: {
    link: "https://discord.gg/UjXeH8uG4x",
  },
  docsRepositoryBase: "https://github.com/golemcloud/docs/blob/master",
  footer: {
    component: <Footer />,
  },
  nextThemes: {
    defaultTheme: "dark",
  },
  useNextSeoProps() {
    const { asPath } = useRouter()
    if (asPath !== "/") {
      return {
        titleTemplate: "%s – Golem Cloud",
      }
    }
  },
  head: (
    <>
      <meta name="viewport" content="width=device-width, initial-scale=1.0" />
      <meta property="og:title" content="Golem Cloud" />
      <meta
        property="og:description"
        content="Learn how to build, deploy, and manage applications on Golem Cloud."
      />
    </>
  ),
}

export default config
