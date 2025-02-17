import Layout from "./components/Layout";
import { BrowserRouter as Router, Routes, Route, Outlet } from "react-router-dom";
import Dashboard from "./pages/Dashboard/Dashboard";
import Overview from "./pages/Overview/Overview";
import Api from "./pages/Api/Api";
import Component from "./pages/Components/Component";
import Plugins from "@pages/Plugin/Plugin";
import OverviewApi from "@pages/Api/Overview";
import APISLayout from "@components/apis/layout";

function App() {
  return (
    <Router>
      <Layout>
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/overview" element={<Overview />} />
          <Route path="/apis" element={<Api />} />
          {/* Use APISLayout with Outlet for nested routes */}
          <Route path="/apis/:id" element={<APISLayout />}>
            <Route path="overview" element={<OverviewApi />} />
          </Route>
          <Route path="/components" element={<Component />} />
          <Route path="/plugins" element={<Plugins />} />
        </Routes>
      </Layout>
    </Router>
  );
}

export default App;