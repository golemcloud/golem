import { Head } from "nextra/components"
import "nextra-theme-docs/style-prefixed.css"
import "../styles/globals.css"
import { Inter } from "next/font/google"
import { DevWarningFilter } from "@/components/dev-warning-filter"

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
        <DevWarningFilter />
        {children}
      </body>
    </html>
  )
}
