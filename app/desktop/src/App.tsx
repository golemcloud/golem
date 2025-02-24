import { lazy, Suspense } from "react";
// BrowserRouter is used for client-side routing
import { BrowserRouter as Router, Route, Routes } from "react-router-dom";
// ThemeProvider provides theming support
import { ThemeProvider } from "@/components/theme-provider.tsx";
import Navbar from "@/components/navbar.tsx";
import ErrorBoundary from "@/components/errorBoundary";
import { Dashboard } from "@/pages/dashboard";
import FileManager from "@/pages/components/details/file.tsx";
import WorkerInfo from "@/pages/workers/details/info.tsx";
import WorkerInvoke from "@/pages/workers/details/invoke.tsx";
import ComponentInvoke from "@/pages/components/details/invoke.tsx";

// Lazy load route components for code splitting and performance improvement
// Lazy-loading improves initial load times by loading components only when needed.
const Components = lazy(() => import("@/pages/components"));
const CreateComponent = lazy(() => import("@/pages/components/create"));
const APIs = lazy(() =>
  import("@/pages/api").then(module => ({ default: module.APIs })),
);
const CreateAPI = lazy(() => import("@/pages/api/create"));
const APIDetails = lazy(() => import("@/pages/api/details"));
const APISettings = lazy(() => import("@/pages/api/details/settings"));
const CreateRoute = lazy(() => import("@/pages/api/details/createRoute.tsx"));
const Deployments = lazy(() => import("@/pages/deployment"));
const ComponentDetails = lazy(() =>
  import("@/pages/components/details").then(module => ({
    default: module.ComponentDetails,
  })),
);
const PluginList = lazy(() => import("@/pages/plugin"));
const ComponentSettings = lazy(
  () => import("@/pages/components/details/settings"),
);
const ComponentInfo = lazy(() => import("@/pages/components/details/info"));
const Exports = lazy(() => import("@/pages/components/details/export"));
const ComponentUpdate = lazy(() => import("@/pages/components/details/update"));
const WorkerList = lazy(() => import("@/pages/workers"));
const APINewVersion = lazy(() => import("./pages/api/details/newVersion"));
const CreateWorker = lazy(() => import("@/pages/workers/create"));
const WorkerDetails = lazy(() => import("@/pages/workers/details"));
const WorkerEnvironments = lazy(
  () => import("@/pages/workers/details/environments"),
);
const WorkerManage = lazy(() => import("@/pages/workers/details/manage"));
const WorkerLive = lazy(() => import("@/pages/workers/details/live"));
const CreatePlugin = lazy(() => import("@/pages/plugin/create"));
const PluginView = lazy(() =>
  import("@/pages/plugin/view").then(module => ({
    default: module.PluginView,
  })),
);
const ApiRoute = lazy(() =>
  import("@/pages/api/details/viewRoute").then(module => ({
    default: module.ApiRoute,
  })),
);
const CreateDeployment = lazy(() => import("@/pages/deployment/create"));
const ApiLayout = lazy(() =>
  import("./pages/api/details/apis-layout").then(module => ({
    default: module.ApiLayout,
  })),
);
const Plugins = lazy(() => import("@/pages/components/details/plugin"));
const ComponentLayout = lazy(() =>
  import("@/pages/components/details/component-layout").then(module => ({
    default: module.ComponentLayout,
  })),
);
const WorkerLayout = lazy(() =>
  import("@/pages/workers/details/worker-layout").then(module => ({
    default: module.WorkerLayout,
  })),
);

// Route configuration constants for ease of maintenance
const ROUTES = {
  DASHBOARD: "/",
  COMPONENTS: "/components",
  COMPONENTS_CREATE: "/components/create",
  COMPONENTS_DETAIL: "/components/:componentId",
  APIS: "/apis",
  APIS_CREATE: "/apis/create",
  APIS_DETAIL: "/apis/:apiName/version/:version",
  DEPLOYMENTS: "/deployments",
  DEPLOYMENTS_CREATE: "/deployments/create",
  PLUGINS: "/plugins",
  PLUGINS_CREATE: "/plugins/create",
  PLUGINS_DETAIL: "/plugins/:pluginId",
  PLUGINS_VERSION: "/plugins/:pluginId/:version",
};

function App() {
  return (
    <ThemeProvider defaultTheme="system" storageKey="golem-theme">
      <Router>
        <div className="min-h-screen">
          {/* Wrap Navbar with ErrorBoundary to catch errors in navigation */}
          <ErrorBoundary>
            <Navbar />
          </ErrorBoundary>
          {/* Suspense provides a fallback UI while lazy-loaded components are being fetched */}
          <Suspense
            fallback={
              <div className="flex items-center justify-center min-h-screen">
                Loading...
              </div>
            }
          >
            <Routes>
              <Route path={ROUTES.DASHBOARD} element={<Dashboard />} />
              <Route path={ROUTES.COMPONENTS} element={<Components />} />
              <Route
                path={ROUTES.COMPONENTS_CREATE}
                element={<CreateComponent />}
              />
              <Route
                path={ROUTES.COMPONENTS_DETAIL}
                element={<ComponentLayout />}
              >
                <Route path="" element={<ComponentDetails />} />
                <Route path="settings" element={<ComponentSettings />} />
                <Route path="update" element={<ComponentUpdate />} />
                <Route path="info" element={<ComponentInfo />} />
                <Route path="exports" element={<Exports />} />
                <Route path="plugins" element={<Plugins />} />
                <Route path="files" element={<FileManager />} />
                <Route path="invoke" element={<ComponentInvoke />} />
                <Route path="workers" element={<WorkerList />} />
                <Route path="workers/create" element={<CreateWorker />} />
              </Route>
              <Route
                path={ROUTES.COMPONENTS_DETAIL + "/workers/:workerName"}
                element={<WorkerLayout />}
              >
                <Route path="" element={<WorkerDetails />} />
                <Route path="environments" element={<WorkerEnvironments />} />
                <Route path="info" element={<WorkerInfo />} />
                <Route path="manage" element={<WorkerManage />} />
                <Route path="invoke" element={<WorkerInvoke />} />
                <Route path="live" element={<WorkerLive />} />
              </Route>

              <Route path={ROUTES.APIS} element={<APIs />} />
              <Route path={ROUTES.APIS_CREATE} element={<CreateAPI />} />

              <Route path={ROUTES.APIS_DETAIL} element={<ApiLayout />}>
                <Route path="" element={<APIDetails />} />
                <Route path="settings" element={<APISettings />} />
                <Route
                  path="routes/add"
                  element={<CreateRoute key="create" />}
                />
                <Route
                  path="routes/edit"
                  element={<CreateRoute key="edit" />}
                />
                <Route path="newversion" element={<APINewVersion />} />
                <Route path="routes" element={<ApiRoute />} />
              </Route>

              <Route path={ROUTES.DEPLOYMENTS} element={<Deployments />} />
              <Route
                path={ROUTES.DEPLOYMENTS_CREATE}
                element={<CreateDeployment />}
              />
              <Route path={ROUTES.PLUGINS} element={<PluginList />} />
              <Route path={ROUTES.PLUGINS_CREATE} element={<CreatePlugin />} />
              <Route path={ROUTES.PLUGINS_DETAIL} element={<PluginView />} />
              <Route path={ROUTES.PLUGINS_VERSION} element={<PluginView />} />
            </Routes>
          </Suspense>
        </div>
      </Router>
    </ThemeProvider>
  );
}

export default App;
