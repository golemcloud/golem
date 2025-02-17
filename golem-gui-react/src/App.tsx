import Layout from "./components/Layout";
import { BrowserRouter as Router, Routes, Route, Outlet } from "react-router-dom";
import Dashboard from "./pages/Dashboard/Dashboard";
import Overview from "./pages/Overview/Overview";
import Api from "./pages/Api/Api";
import Component from "./pages/Components/Component";
import Plugins from "@pages/Plugin/Plugin";
import OverviewApi from "@pages/Api/Overview";
import ApiLayout from "@components/apis/layout";
import NewRoute from "@pages/Api/NewRoute";
import Playground from "@pages/Api/Playground";
import PlaygroundLayout from "@components/apis/playground/layout";
import RouteInfo from "@pages/Api/RouteInfo";
import Deployment from "@pages/Api/Deployment";
import Settings from "@pages/Api/Settings";
import ComponentsLayout from "@components/components/layout";
import OverviewDurable from "@pages/Components/OverviewDurable";
import OverviewEphemeral from "@pages/Components/OverviewEphemeral";
import ExportsPage from "@pages/Components/Exports";
import FilesPage from "@pages/Components/Files";
import SettingsPage from "@pages/Components/Settings";
import WorkerPage from "@pages/Components/Worker";

function App() {
  return (
    <Router>
      <Layout>
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/overview" element={<Overview />} />
          <Route path="/apis" element={<Api />} />
          <Route path="/apis/:apiId" element={<ApiLayout />}>
            <Route path="overview" element={<OverviewApi />} />
            <Route path="new-route" element={<NewRoute />}/>
            <Route path="playground" element={<PlaygroundLayout />}>
              <Route index element={<Playground />} />
            </Route>
            <Route path="deployments" element={<Deployment />} />
            <Route path="settings" element={<Settings />} />
            {/* Move the dynamic route to the end */}
            <Route path=":routeId" element={<RouteInfo />} />
          </Route>
          <Route path="/components" element={<Component />} />
          <Route path="/components/:compId" element={<ComponentsLayout />} >
             <Route path="durableoverview" element={ <OverviewDurable/>}/>
             <Route path="ephemeraloverview" element={ <OverviewEphemeral/>}/>
             <Route path="exports" element={ <ExportsPage/>}/>
              <Route path="files" element={ <FilesPage/>}/>
              <Route path="settings" element={ <SettingsPage/>}/>
              <Route path="workers" element={ <WorkerPage/>}/>
          </Route>
          <Route path="/plugins" element={<Plugins />} />
        </Routes>
      </Layout>
    </Router>
  );
}

export default App;