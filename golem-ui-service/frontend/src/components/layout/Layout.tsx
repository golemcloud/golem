import {
  ChevronRight,
  Menu,
  Package,
  Puzzle,
  TableOfContents,
  Webhook,
  X,
} from "lucide-react";
import { Link, useLocation } from "react-router-dom";
import React, { useState } from "react";

const navItems = [
  { label: "Overview", path: "/", icon: TableOfContents },
  { label: "Components", path: "/components", icon: Package },
  { label: "Plugins", path: "/plugins", icon: Puzzle },
  { label: "API", path: "/api", icon: Webhook },
];

const NavLink = ({
  item,
  isActive,
}: {
  item: (typeof navItems)[0];
  isActive: boolean;
}) => {
  const Icon = item.icon;

  return (
    <Link
      to={item.path}
      className={`flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium
                 transition-all duration-200 group ${
                   isActive
                     ? "bg-blue-500/10 text-blue-400"
                     : "text-gray-400 hover:text-gray-200 hover:bg-gray-800"
                 }`}
    >
      <Icon
        size={18}
        className={`${isActive ? "text-blue-400" : "text-gray-500"} 
                                 transition-colors group-hover:text-inherit`}
      />
      <span>{item.label}</span>
      {isActive && <ChevronRight size={16} className="ml-auto text-blue-400" />}
    </Link>
  );
};

export const Layout = ({ children }: { children: React.ReactNode }) => {
  const location = useLocation();
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);

  return (
    <div className="min-h-screen bg-gray-900 text-gray-100">
      {/* Header */}
      <header
        className="sticky top-0 z-50 bg-gray-800/80 border-b border-gray-700/50 
                        backdrop-blur supports-[backdrop-filter]:bg-gray-800/60"
      >
        <div className="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <div className="flex h-16 items-center justify-between">
            {/* Logo */}
            <Link to="/" className="flex items-center gap-3 group">
              <div
                className="p-2 rounded-md bg-blue-500/10 text-blue-400 
                            transition-colors group-hover:bg-blue-500/20"
              >
                <Webhook size={22} />
              </div>
              <span className="text-xl font-bold text-white">
                Golem <span className="text-blue-400">UI</span>
              </span>
            </Link>

            {/* Desktop Navigation */}
            <nav className="hidden md:flex items-center space-x-2">
              {navItems.map((item) => (
                <NavLink
                  key={item.path}
                  item={item}
                  isActive={location.pathname === item.path}
                />
              ))}
            </nav>

            {/* Mobile menu button */}
            <button
              onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
              className="md:hidden p-2 text-gray-400 hover:text-gray-200 
                       hover:bg-gray-800 rounded-lg transition-colors"
            >
              {mobileMenuOpen ? <X size={24} /> : <Menu size={24} />}
            </button>
          </div>
        </div>

        {/* Mobile Navigation */}
        {mobileMenuOpen && (
          <nav className="md:hidden border-t border-gray-700/50 bg-gray-800">
            <div className="space-y-1 px-4 py-3">
              {navItems.map((item) => (
                <NavLink
                  key={item.path}
                  item={item}
                  isActive={location.pathname === item.path}
                />
              ))}
            </div>
          </nav>
        )}
      </header>

      {/* Main content */}
      <main className="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8 py-6 min-h-[calc(100vh-4rem)]">
        {children}
      </main>

      {/* Footer */}
      <footer className="border-t border-gray-800 bg-gray-900/50 py-6 mt-auto">
        <div className="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <div className="flex items-center justify-between">
            <div className="text-sm text-gray-500">
              © {new Date().getFullYear()} Golem Cloud. All rights reserved.
            </div>
            <div className="flex items-center gap-4">
              <a
                href="https://docs.golem.cloud"
                target="_blank"
                rel="noopener noreferrer"
                className="text-sm text-gray-500 hover:text-gray-400 transition-colors"
              >
                Documentation
              </a>
              <a
                href="https://github.com/golemcloud"
                target="_blank"
                rel="noopener noreferrer"
                className="text-sm text-gray-500 hover:text-gray-400 transition-colors"
              >
                GitHub
              </a>
            </div>
          </div>
        </div>
      </footer>
    </div>
  );
};
