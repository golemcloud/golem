import { Outlet, useParams } from "react-router-dom";
import { Suspense } from "react";
import ErrorBoundary from "@/components/errorBoundary";
import Navbar from "@/components/navbar.tsx";

export const AppLayout = () => {
  const { id } = useParams();

  return (
    <>
      <ErrorBoundary>
        <Navbar />
      </ErrorBoundary>
      <Suspense
        fallback={
          <div className="flex items-center justify-center min-h-screen">
            Loading app {id}...
          </div>
        }
      >
        <Outlet />
      </Suspense>
    </>
  );
};

export default AppLayout;
