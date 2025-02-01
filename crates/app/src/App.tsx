import { BrowserRouter as Router, Route, Routes } from "react-router-dom";
import Components from "@/pages/components";
import CreateComponent from "@/pages/components/create";
import { APIs } from "@/pages/api";
import CreateAPI from "@/pages/api/create";
import APIDetails from "@/pages/api/details";
import APISettings from "@/pages/api/details/settings";
import CreateRoute from "@/pages/api/details/createRoute.tsx";
import Deployments from "@/pages/deployment";
import { ComponentDetails } from "@/pages/components/details";
import { PluginList } from "@/pages/plugin";
import ComponentSettings from "@/pages/components/details/settings";
import ComponentInfo from "@/pages/components/details/info";
import Exports from "@/pages/components/details/export";
import ComponentUpdate from "@/pages/components/details/update";
import WorkerList from "@/pages/workers";
import { ThemeProvider } from "@/components/theme-provider.tsx";
import Navbar from "@/components/navbar.tsx";
import APINewVersion from "./pages/api/details/newVersion";
import { Dashboard } from "@/pages/dashboard";
import CreateWorker from "@/pages/workers/create";
import WorkerDetails from "@/pages/workers/details";
import ErrorBoundary from "@/components/errorBoundary";
import WorkerEnvironments from "@/pages/workers/details/environments";
import WorkerManage from "@/pages/workers/details/manage";
import WorkerInvoke from "@/pages/workers/details/invoke";
import WorkerLive from "@/pages/workers/details/live";
import CreatePlugin from "@/pages/plugin/create.tsx";
import { PluginView } from "@/pages/plugin/view.tsx";
import { ApiRoute } from "@/pages/api/details/viewRoute";
import CreateDeployment from "@/pages/deployment/create";
import { ApiLayout } from "./pages/api/details/api-layout";
import ComponentInvoke from "@/pages/components/details/invoke";

function App() {
  return (
    <ThemeProvider defaultTheme="system" storageKey="golem-theme">
      <Router>
        <div className="min-h-screen">
          <ErrorBoundary>
            <Navbar />
          </ErrorBoundary>
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/components" element={<Components />} />
            <Route path="/components/create" element={<CreateComponent />} />
            <Route
              path="/components/:componentId"
              element={<ComponentDetails />}
            />
            <Route
              path="/components/:componentId/settings"
              element={<ComponentSettings />}
            />
            <Route
              path="/components/:componentId/update"
              element={<ComponentUpdate />}
            />
            <Route
              path="/components/:componentId/info"
              element={<ComponentInfo />}
            />
            <Route
              path="/components/:componentId/exports"
              element={<Exports />}
            />
            <Route
              path="/components/:componentId/invoke"
              element={<ComponentInvoke />}
            />
            <Route
              path="/components/:componentId/workers"
              element={<WorkerList />}
            />
            <Route
              path="/components/:componentId/workers/create"
              element={<CreateWorker />}
            />
            <Route
              path="/components/:componentId/workers/:workerName"
              element={<WorkerDetails />}
            />
            <Route
              path="/components/:componentId/workers/:workerName/environments"
              element={<WorkerEnvironments />}
            />
            <Route
              path="/components/:componentId/workers/:workerName/manage"
              element={<WorkerManage />}
            />
            <Route
              path="/components/:componentId/workers/:workerName/invoke"
              element={<WorkerInvoke />}
            />
            <Route
              path="/components/:componentId/workers/:workerName/live"
              element={<WorkerLive />}
            />
            <Route path="/apis" element={<APIs />} />
            <Route path="/apis/create" element={<CreateAPI />} />
            <Route
              path="/apis/:apiName/version/:version"
              element={<ApiLayout />}
            >
              <Route path="" element={<APIDetails />} />
              <Route path="settings" element={<APISettings />} />
              <Route path="routes/add" element={<CreateRoute key="create" />} />
              <Route path="routes/edit" element={<CreateRoute key="edit" />} />
              <Route path="newversion" element={<APINewVersion />} />
              <Route path="routes" element={<ApiRoute />} />
            </Route>
            <Route path="/deployments" element={<Deployments />} />
            <Route path="/plugins" element={<PluginList />} />
            <Route path="/plugins/create" element={<CreatePlugin />} />
            <Route path="/deployments" element={<Deployments />} />
            <Route path="/deployments/create" element={<CreateDeployment />} />
            <Route path="/plugins" element={<PluginList />} />
            <Route path="/plugins/create" element={<CreatePlugin />} />
            <Route path="/plugins/:pluginId" element={<PluginView />} />
            <Route
              path="/plugins/:pluginId/:version"
              element={<PluginView />}
            />
          </Routes>
        </div>
      </Router>
    </ThemeProvider>
  );
}

export default App;
