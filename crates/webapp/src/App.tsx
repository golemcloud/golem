import { BrowserRouter as Router, Routes, Route } from 'react-router-dom';
import Navbar from './components/Navbar';
import Overview from './pages/overview';
import Components from './pages/components';
import CreateComponent from './pages/components/create';
import {APIs} from './pages/api';
import CreateAPI from './pages/api/create';
import APIDetails from './pages/api/details';
import APISettings from './pages/api/details/settings';
import CreateRoute from './pages/api/details/CreateRoute';
import Deployments from './pages/Deployments';
import Plugin from "./pages/plugin";
import ComponentDetails from "./pages/components/details";

function App() {
    return (
        <Router>
            <div className="min-h-screen bg-gray-50">
                <Navbar />
                <Routes>
                    <Route path="/" element={<Overview />} />
                    <Route path="/components" element={<Components />} />
                    <Route path="/components/create" element={<CreateComponent />} />
                    <Route path="/components/:componentId" element={<ComponentDetails />} />
                    <Route path="/apis" element={<APIs />} />
                    <Route path="/apis/create" element={<CreateAPI />} />
                    <Route path="/apis/:apiName" element={<APIDetails />} />
                    <Route path="/apis/:apiName/settings" element={<APISettings />} />
                    <Route path="/apis/:apiName/routes/new" element={<CreateRoute />} />
                    <Route path="/deployments" element={<Deployments />} />
                    <Route path="/plugins" element={<Plugin />} />
                </Routes>
            </div>
        </Router>
    );
}

export default App;