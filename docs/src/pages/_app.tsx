import "../styles/globals.css"

import type { AppProps } from "next/app"

import { Inter } from "next/font/google"

const font = Inter({
  subsets: ["latin"],
  display: "swap",
  variable: "--font-sans",
})

export default function MyApp({ Component, pageProps }: AppProps) {
  return (
    <div
      className={`${font.variable} bg-background-light font-sans dark:bg-background-dark`}
      style={{
        // Alternate t style for Satoshi.
        fontFeatureSettings: '"ss03" 1',
      }}
    >
      <Component {...pageProps} />
    </div>
  )
}
