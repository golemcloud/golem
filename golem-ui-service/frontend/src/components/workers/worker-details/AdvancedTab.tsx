import {
  Pause,
  Play,
  Power,
  PowerOff,
  Puzzle,
  RefreshCw,
  Shield,
  Trash,
} from "lucide-react";
import { PluginInstall, Worker } from "../../../types/api";
import React, { useState } from "react";

import { UpdateVersionModal } from "./UpdateVersionModal";
import { useInstalledPlugins } from "../../../api/plugins";

interface AdvancedTabProps {
  worker: Worker;
  onAction: (
    action:
      | "interrupt"
      | "resume"
      | "delete"
      | "activate-plugin"
      | "deactivate-plugin"
      | "update-version",
    pluginID?: string,
  ) => void;
}

const AdvancedTab: React.FC<AdvancedTabProps> = ({ worker, onAction }) => {
  const [showUpdateModal, setShowUpdateModal] = useState(false);
  const {
    data: plugins,
    isLoading: isLoadingPlugins,
  }: {
    data: PluginInstall[];
    isLoading: boolean;
  } = useInstalledPlugins(worker.workerId.componentId, worker.componentVersion);

  // Filter plugins that match the worker's component
  const compatiblePlugins = plugins;

  return (
    <div className="space-y-6">
      {/* Worker Controls */}
      <div className="bg-card/80 border border-border/10 rounded-lg p-6">
        <h3 className="text-lg font-semibold flex items-center gap-2 mb-4">
          <Shield size={20} className="text-primary" />
          Worker Controls
        </h3>
        <div className="space-y-4">
          <button
            onClick={() => onAction("interrupt")}
            className="w-full flex items-center justify-between p-4 bg-card/60 rounded-lg hover:bg-card/70 transition-colors disabled:opacity-50"
          >
            <div className="flex items-center gap-3">
              <Pause size={16} className="text-destructive" />
              <div className="text-left">
                <div className="font-medium">Interrupt Worker</div>
                <div className="text-sm text-muted-foreground">
                  Temporarily pause worker execution
                </div>
              </div>
            </div>
          </button>

          <button
            onClick={() => onAction("resume")}
            className="w-full flex items-center justify-between p-4 bg-card/60 rounded-lg hover:bg-card/70 transition-colors disabled:opacity-50"
          >
            <div className="flex items-center gap-3">
              <Play size={16} className="text-success" />
              <div className="text-left">
                <div className="font-medium">Resume Worker</div>
                <div className="text-sm text-muted-foreground">
                  Resume worker execution
                </div>
              </div>
            </div>
          </button>

          <button
            onClick={() => setShowUpdateModal(true)}
            className="w-full flex items-center justify-between p-4 bg-card/60 rounded-lg hover:bg-card/70 transition-colors disabled:opacity-50 group"
          >
            <div className="flex items-center gap-3">
              <RefreshCw size={16} className="text-primary group-hover:rotate-180 transition-transform duration-300" />
              <div className="text-left">
                <div className="font-medium">Update Version</div>
                <div className="text-sm text-muted-foreground">
                  Change component version for this worker
                </div>
              </div>
            </div>
          </button>

          <button
            onClick={() => {
              if (
                confirm(
                  "Are you sure you want to delete this worker? This action cannot be undone.",
                )
              ) {
                onAction("delete");
              }
            }}
            className="w-full flex items-center justify-between p-4 bg-destructive/10 rounded-lg hover:bg-destructive/20 transition-colors"
          >
            <div className="flex items-center gap-3">
              <Trash size={16} className="text-destructive" />
              <div className="text-left">
                <div className="font-medium">Delete Worker</div>
                <div className="text-sm text-muted-foreground">
                  Permanently delete this worker
                </div>
              </div>
            </div>
          </button>
        </div>
      </div>

      {/* Plugins Section */}
      <div className="bg-card/80 border border-border/10 rounded-lg p-6">
        <h3 className="text-lg font-semibold flex items-center gap-2 mb-4">
          <Puzzle size={20} className="text-primary" />
          Plugin Management
        </h3>

        {isLoadingPlugins ? (
          <div className="text-center py-4 text-muted-foreground">
            Loading plugins...
          </div>
        ) : compatiblePlugins.length === 0 ? (
          <div className="text-center py-4 text-muted-foreground">
            No compatible plugins found for this worker
          </div>
        ) : (
          <div className="space-y-4">
            {compatiblePlugins.map((plugin: PluginInstall) => {
              const isActive = worker.activePlugins.includes(plugin.id);

              return (
                <div
                  key={`${plugin.name}-${plugin.version}`}
                  className="flex items-center justify-between p-4 bg-card/60 rounded-lg"
                >
                  <div className="flex items-center gap-3">
                    <Puzzle size={16} className="text-primary" />
                    <div>
                      <div className="font-medium">{plugin.name}</div>
                      <div className="text-sm text-muted-foreground">
                        Version {plugin.version}
                      </div>
                    </div>
                  </div>

                  <button
                    onClick={() =>
                      onAction(
                        isActive ? "deactivate-plugin" : "activate-plugin",
                        plugin.id,
                      )
                    }
                    className={`flex items-center gap-2 px-4 py-2 rounded-md transition-colors ${
                      isActive
                        ? "bg-destructive/10 hover:bg-destructive/20 text-destructive"
                        : "bg-success/10 hover:bg-success/20 text-success"
                    }`}
                  >
                    {isActive ? (
                      <>
                        <PowerOff size={16} />
                        <span>Deactivate</span>
                      </>
                    ) : (
                      <>
                        <Power size={16} />
                        <span>Activate</span>
                      </>
                    )}
                  </button>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Version Update Modal */}
      <UpdateVersionModal
        isOpen={showUpdateModal}
        onClose={() => setShowUpdateModal(false)}
        worker={worker}
      />
    </div>
  );
};

export default AdvancedTab;