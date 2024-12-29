import './globals.css';
import { Inter } from 'next/font/google';
import Navbar from '@/components/ui/Navbar';
import { ThemeProvider } from "@/components/theme-provider"
import Footer from "@/components/ui/Footer"
import { ToastContainer } from 'react-toastify';

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
            <ToastContainer
              position="bottom-right"
              theme='dark'
            />
          <Navbar />
          <div style={{ display: 'flex', minHeight:'100vh' ,maxWidth:"75%" , margin:"auto", flexDirection: 'column', }}
            className='container mx-auto flex flex-col gap-8 px-4 py-8 md:px-6 lg:px-8'
          >
            <main style={{ flexGrow: 1, }}>{children}</main>
          </div>
          <Footer/>
        </ThemeProvider>
      </body>
    </html>
  );
}
