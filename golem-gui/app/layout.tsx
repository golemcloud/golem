import './globals.css';
import { Inter } from 'next/font/google';
import Navbar from '@/components/ui/Navbar';
import { ThemeProvider } from "@/components/theme-provider"
import Footer from "@/components/ui/Footer"

const inter = Inter({ subsets: ['latin'] });

export const metadata = {
  title: 'Golem UI',
  description: 'Graphical UI for managing Golem APIs',
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en">
      <body className={inter.className}>
        <ThemeProvider
            attribute="class"
            defaultTheme="system"
            enableSystem
            disableTransitionOnChange
          >
          <Navbar />
          <div style={{ display: 'flex' }}>
            <main style={{ flexGrow: 1, }}>{children}</main>
          </div>
          <Footer/>
        </ThemeProvider>
      </body>
    </html>
  );
}
