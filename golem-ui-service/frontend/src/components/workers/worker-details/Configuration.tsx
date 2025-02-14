import {
  AlertCircle,
  Code2,
  FileText,
  Layers,
  Link2,
  RefreshCw,
  Settings,
  Terminal,
} from "lucide-react";

import React from "react";
import { UpdateVersionModal } from "./UpdateVersionModal";
import { Worker } from "../../../types/api";

interface ConfigTabProps {
  worker: Worker;
}

const ConfigTab: React.FC<ConfigTabProps> = ({ worker }) => {
  const [showUpdateModal, setShowUpdateModal] = React.useState(false);
  return (
    <div className="space-y-6">
      {/* Basic Configuration */}
      <div className="bg-card/80 border border-border/10 rounded-lg p-6">
        <h3 className="text-lg font-semibold flex items-center gap-2 mb-4">
          <Settings size={20} className="text-primary" />
          Basic Configuration
        </h3>
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-sm font-medium text-muted-foreground mb-1">
              Worker ID
            </label>
            <div className="p-3 bg-card/60 rounded-lg font-mono text-sm">
              {worker.workerId.workerName}
            </div>
          </div>
          {/* <div>
            <label className="block text-sm font-medium text-muted-foreground mb-1">
              Component Version
            </label>
            <div className="p-3 bg-card/60 rounded-lg font-mono text-sm">
              {worker.componentVersion}
            </div>
          </div> */}
          <div>
            <label className="block text-sm font-medium text-muted-foreground mb-1">
              Component Version
            </label>
            <div className="flex items-center gap-2">
              <div className="p-3 bg-card/60 rounded-lg font-mono text-sm flex-1">
                {worker.componentVersion}
              </div>
              <button
                onClick={() => setShowUpdateModal(true)}
                className="p-2 text-muted-foreground hover:text-primary rounded-md 
        hover:bg-card/60 transition-colors"
                title="Update Version"
              >
                <RefreshCw size={16} />
              </button>
            </div>
          </div>
        </div>
      </div>

      <div className="grid grid-cols-2 gap-6">
        {/* Resources */}
        <div className="bg-card/80 border border-border/10 rounded-lg p-6">
          <h3 className="text-lg font-semibold flex items-center gap-2 mb-4">
            <Layers size={20} className="text-primary" />
            Owned Resources
          </h3>
          <div className="space-y-2">
            {Object.entries(worker.ownedResources).map(([key, resource]) => (
              <div key={key} className="p-3 bg-card/60 rounded-lg">
                <div className="flex items-center justify-between mb-2">
                  <span className="font-medium">
                    {resource.indexed.resourceName}
                  </span>
                  <span className="text-sm text-muted-foreground">
                    {new Date(resource.createdAt).toLocaleDateString()}
                  </span>
                </div>
                {resource.indexed.resourceParams.map((param, index) => (
                  <div
                    key={index}
                    className="text-sm text-muted-foreground ml-4"
                  >
                    <Link2 size={14} className="inline mr-2" />
                    {param}
                  </div>
                ))}
              </div>
            ))}
            {Object.keys(worker.ownedResources).length === 0 && (
              <div className="text-center py-6 text-muted-foreground">
                No resources owned by this worker
              </div>
            )}
          </div>
        </div>

        {/* Active Plugins */}
        <div className="bg-card/80 border border-border/10 rounded-lg p-6">
          <h3 className="text-lg font-semibold flex items-center gap-2 mb-4">
            <FileText size={20} className="text-primary" />
            Active Plugins
          </h3>
          <div className="grid grid-cols-2 gap-4">
            {worker.activePlugins.map((plugin) => (
              <div key={plugin} className="p-4 bg-card/60 rounded-lg">
                <div className="font-medium">{plugin}</div>
              </div>
            ))}
            {worker.activePlugins.length === 0 && (
              <div className="col-span-2 text-center py-6 text-muted-foreground">
                No active plugins
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Environment Variables */}
      <div className="bg-card/80 border border-border/10 rounded-lg p-6">
        <h3 className="text-lg font-semibold flex items-center gap-2 mb-4">
          <Code2 size={20} className="text-primary" />
          Environment Variables
        </h3>
        <div className="space-y-2">
          {Object.entries(worker.env).map(([key, value]) => (
            <div
              key={key}
              className="flex items-center justify-between p-3 bg-card/60 rounded-lg"
            >
              <span className="font-mono text-sm text-muted-foreground">
                {key}
              </span>
              <span className="font-mono text-sm">{value}</span>
            </div>
          ))}
          {Object.keys(worker.env).length === 0 && (
            <div className="text-center py-6 text-muted-foreground">
              No environment variables set
            </div>
          )}
        </div>
      </div>

      {/* Arguments Configuration */}
      <div className="bg-card/80 border border-border/10 rounded-lg p-6">
        <h3 className="text-lg font-semibold flex items-center gap-2 mb-4">
          <Terminal size={20} className="text-primary" />
          Arguments
        </h3>
        <div className="p-3 bg-card/60 rounded-lg font-mono text-sm">
          {worker.args.length > 0 ? (
            worker.args.map((arg, index) => (
              <div key={index} className="mb-1 last:mb-0">
                {arg}
              </div>
            ))
          ) : (
            <span className="text-muted-foreground">No arguments</span>
          )}
        </div>
      </div>

      {/* Error Information */}
      {worker.lastError && (
        <div className="bg-card/80 border border-destructive/20 rounded-lg p-6">
          <h3 className="text-lg font-semibold flex items-center gap-2 mb-4 text-destructive">
            <AlertCircle size={20} />
            Last Error
          </h3>
          <div className="p-3 bg-destructive/10 text-destructive rounded-lg font-mono text-sm">
            {worker.lastError}
          </div>
        </div>
      )}

      {showUpdateModal && (
        <UpdateVersionModal
          isOpen={showUpdateModal}
          onClose={() => setShowUpdateModal(false)}
          worker={worker}
        />
      )}
    </div>
  );
};

export default ConfigTab;
