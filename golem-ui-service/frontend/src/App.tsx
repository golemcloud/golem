import { BrowserRouter, Route, Routes } from 'react-router-dom';

import { ApiDefinitionView } from './pages/ApiDefinitionDetail';
import { ApiDefinitionsPage } from './pages/ApiDefinitions';
import {
  ComponentDetail
} from './pages/ComponentDetail';
import { Components } from './pages/Components';
import { Layout } from './components/layout/Layout';
import { Overview } from './pages/Overview';
import PluginDetailPage from './pages/PluginDetail';
import { PluginsPage } from './pages/Plugins';
import { Toaster } from 'react-hot-toast';

function App() {
  return (
    <BrowserRouter>
      <Layout>
        <Routes>
          <Route path="/" element={<Overview />} />
          <Route path="/workers" element={<div>Workers Page</div>} />
          <Route path="/components" element={<Components />} />
          <Route path="/components/:id" element={<ComponentDetail />} />
          <Route path="/plugins" element={<PluginsPage />} />
          <Route path="/api" element={<ApiDefinitionsPage />} />
          <Route path="/api/definitions/:id/:version" element={<ApiDefinitionView />} />
          <Route path="/plugins/:name/:version" element={<PluginDetailPage />} />

          {/* <Route path="/api" */}
        </Routes>
        <div>
          {/* This component will render the toasts */}
          <Toaster
            position="top-right"
            toastOptions={{
              duration: 5000,
              style: {
                background: '#1F2937',
                color: '#F3F4F6',
                border: '1px solid #374151',
              },
              success: {
                icon: '✅',
              },
              error: {
                icon: '❌',
                duration: 7000,
              },
            }}
          />
        </div>
      </Layout>
    </BrowserRouter>
  );
}

export default App;