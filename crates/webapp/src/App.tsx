import { BrowserRouter as Router, Route, Routes } from "react-router-dom";
import Components from "@/pages/components";
import CreateComponent from "@/pages/components/create";
import { APIs } from "@/pages/api";
import CreateAPI from "@/pages/api/create";
import APIDetails from "@/pages/api/details";
import APISettings from "@/pages/api/details/settings";
import CreateRoute from "@/pages/api/details/createRoute.tsx";
import Deployments from "@/pages/Deployments";
import { ComponentDetails } from "@/pages/components/details";
import { PluginList } from "@/pages/plugin";
import ComponentSettings from "@/pages/components/details/settings";
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
import CreatePlugin from "@/pages/plugin/create.tsx";

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
              path="/components/:componentId/exports"
              element={<Exports />}
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
            <Route path="/apis" element={<APIs />} />
            <Route path="/apis/create" element={<CreateAPI />} />
            <Route path="/apis/:apiName" element={<APIDetails />} />
            <Route path="/apis/:apiName/settings" element={<APISettings />} />
            <Route
              path="/apis/:apiName/newversion"
              element={<APINewVersion />}
            />
            <Route path="/apis/:apiName/routes/new" element={<CreateRoute />} />
            <Route path="/deployments" element={<Deployments />} />
            <Route path="/plugins" element={<PluginList />} />
            <Route path="/plugins/create" element={<CreatePlugin />} />
          </Routes>
        </div>
      </Router>
    </ThemeProvider>
  );
}

export default App;
