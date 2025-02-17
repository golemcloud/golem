// src/components/Layout.tsx
import React, { ReactNode } from 'react';
import Navbar from './Navbar';
import Footer from './Footer';
import { ThemeProvider } from './ui/theme-provider';
import { ToastContainer } from 'react-toastify';
import 'react-toastify/dist/ReactToastify.css';

interface LayoutProps {
  children: ReactNode;
}

const Layout: React.FC<LayoutProps> = ({ children }) => {
  return (
    <div className="min-h-screen flex flex-col">
      <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
        <ToastContainer position="bottom-right" theme="dark" />
        <Navbar />
        <div className="flex-1">{children}</div>
        <Footer />
      </ThemeProvider>
    </div>
  );
};

export default Layout;
