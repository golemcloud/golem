import { Suspense, useEffect } from "react";
// BrowserRouter is used for client-side routing
import { BrowserRouter as Router, useRoutes } from "react-router-dom";
// ThemeProvider provides theming support
import { ThemeProvider } from "@/components/theme-provider.tsx";
import { LogViewerProvider, useLogViewer } from "@/contexts/log-viewer-context";
import { GlobalLogViewer } from "@/components/global-log-viewer";
import { Toaster } from "@/components/ui/toaster";
import { setupGlobalLogViewer } from "@/hooks/use-toast";
import { appRoutes } from "./routes";

// Component to set up global log viewer reference
const ToastSetup = () => {
  const logViewer = useLogViewer();

  useEffect(() => {
    setupGlobalLogViewer(logViewer);
  }, [logViewer]);

  return null;
};

// AppRoutes component to render routes using useRoutes hook
const AppRoutes = () => {
  const routes = useRoutes(appRoutes);

  return routes;
};

function App() {
  return (
    <ThemeProvider defaultTheme="system" storageKey="golem-theme">
      <LogViewerProvider>
        <ToastSetup />
        <Router>
          <div className="min-h-screen">
            {/* Suspense provides a fallback UI while lazy-loaded components are being fetched */}
            <Suspense
              fallback={
                <div className="flex items-center justify-center min-h-screen">
                  Loading...
                </div>
              }
            >
              <AppRoutes />
            </Suspense>
            <GlobalLogViewer />
            <Toaster />
          </div>
        </Router>
      </LogViewerProvider>
    </ThemeProvider>
  );
}

export default App;
