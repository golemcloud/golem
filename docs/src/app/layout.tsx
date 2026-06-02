import { Footer, Layout, Navbar } from "nextra-theme-docs"
import { Head } from "nextra/components"
import { getPageMap } from "nextra/page-map"
import "nextra-theme-docs/style-prefixed.css"
import "../styles/globals.css"
import { Inter } from "next/font/google"
import { GolemLogo } from "@/components/golem-logo"
import { Footer as GolemFooter } from "@/components/footer"

const font = Inter({
  subsets: ["latin"],
  display: "swap",
  variable: "--font-sans",
})

export const metadata = {
  title: {
    template: "%s – Golem Cloud",
    default: "Golem Cloud",
  },
  description: "Learn how to build, deploy, and manage applications on Golem Cloud.",
}

export default async function RootLayout({ children }: { children: React.ReactNode }) {
  const pageMap = await getPageMap()
  return (
    <html lang="en" dir="ltr" suppressHydrationWarning>
      <Head
        color={{
          hue: { dark: 213, light: 226 },
          saturation: { dark: 93, light: 90 },
        }}
      >
        <meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <meta property="og:title" content="Golem Cloud" />
        <meta
          property="og:description"
          content="Learn how to build, deploy, and manage applications on Golem Cloud."
        />
      </Head>
      <body className={`${font.variable} font-sans`} style={{ fontFeatureSettings: '"ss03" 1' }}>
        <Layout
          navbar={
            <Navbar
              logo={<GolemLogo />}
              projectLink="https://github.com/golemcloud/docs"
              chatLink="https://discord.gg/UjXeH8uG4x"
            />
          }
          footer={<GolemFooter />}
          docsRepositoryBase="https://github.com/golemcloud/docs/blob/main"
          sidebar={{ defaultMenuCollapseLevel: 1, toggleButton: true }}
          pageMap={pageMap}
          nextThemes={{ defaultTheme: "dark" }}
        >
          {children}
        </Layout>
      </body>
    </html>
  )
}
