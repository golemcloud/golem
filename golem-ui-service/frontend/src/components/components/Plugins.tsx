import { Plugin, PluginInstall } from "../../types/api";
import { Puzzle, Save, Sliders, Trash2, X } from "lucide-react";
import { useInstallPlugin, useInstalledPlugins, usePlugins, useUninstallPlugin, useUpdatePluginInstallation } from "../../api/plugins";

import toast from "react-hot-toast";
import { useState } from "react";

export const PluginSection = ({ componentId, version }: { componentId: string, version: number }) => {
    const { data: availablePlugins } = usePlugins();
    const { data: installedPlugins } = useInstalledPlugins(componentId, version);
    const installPlugin = useInstallPlugin(componentId, version);
    const uninstallPlugin = useUninstallPlugin(componentId, version);
    const updatePluginInstallation = useUpdatePluginInstallation(componentId, version);
    
    const [editingPriority, setEditingPriority] = useState<{
        id: string;
        priority: number;
    } | null>(null);

    const handleInstallPlugin = async (plugin: Plugin) => {
        try {
            await installPlugin.mutateAsync({
                name: plugin.name,
                version: plugin.version,
                priority: 0,
                parameters: {}
            });
            toast.success("Plugin installed successfully");
        } catch (error) {
            toast.error("Failed to install plugin");
        }
    };

    const handleUninstallPlugin = async (installationId: string) => {
        if (!window.confirm("Are you sure you want to uninstall this plugin?")) {
            return;
        }

        try {
            await uninstallPlugin.mutateAsync(installationId);
            toast.success("Plugin uninstalled successfully");
        } catch (error) {
            toast.error("Failed to uninstall plugin");
        }
    };

    const handlePriorityChange = (value: string) => {
        if (editingPriority) {
            const newPriority = parseInt(value);
            if (!isNaN(newPriority)) {
                setEditingPriority({ ...editingPriority, priority: newPriority });
            }
        }
    };

    const handleSavePriority = async () => {
        if (!editingPriority) return;

        try {
            await updatePluginInstallation.mutateAsync({
                installationId: editingPriority.id,
                payload: {
                    priority: editingPriority.priority,
                    parameters: {}
                }
            });
            toast.success("Plugin priority updated successfully");
            setEditingPriority(null);
        } catch (error) {
            toast.error("Failed to update plugin priority");
        }
    };

    const handleResetPriority = async (installationId: string) => {
        if (!window.confirm("Are you sure you want to reset this plugin's priority?")) {
            return;
        }

        try {
            await updatePluginInstallation.mutateAsync({
                installationId,
                payload: {
                    priority: 0,
                    parameters: {}
                }
            });
            toast.success("Plugin priority reset successfully");
        } catch (error) {
            toast.error("Failed to reset plugin priority");
        }
    };

    return (
        <div className="bg-card rounded-lg p-4 md:p-6">
            <h2 className="text-lg md:text-xl font-semibold mb-4 md:mb-6 flex items-center gap-3">
                <Puzzle size={20} className="text-primary" />
                Plugins
            </h2>

            {/* Installed Plugins */}
            <div className="mb-6">
                <h3 className="text-sm font-medium text-muted-foreground mb-3">Installed Plugins</h3>
                <div className="space-y-3">
                    {installedPlugins?.map((plugin: PluginInstall) => (
                        <div key={plugin.id}
                            className="group flex items-center justify-between p-3 bg-card/50 rounded-lg hover:bg-card/80 transition-colors">
                            <div className="flex items-center gap-3">
                                <Puzzle size={16} className="text-primary" />
                                <div>
                                    <div className="font-medium text-sm">{plugin.name}</div>
                                    <div className="text-xs text-muted-foreground flex items-center gap-2">
                                        Version {plugin.version} •
                                        {editingPriority?.id === plugin.id ? (
                                            <div className="flex items-center gap-2">
                                                <input
                                                    type="number"
                                                    value={editingPriority.priority}
                                                    onChange={(e) => handlePriorityChange(e.target.value)}
                                                    className="w-16 px-2 py-1 text-xs bg-card rounded border border-primary focus:outline-none"
                                                    min="0"
                                                />
                                                <button
                                                    onClick={handleSavePriority}
                                                    className="p-1 text-primary hover:text-primary/80 transition-colors"
                                                    title="Save Priority"
                                                >
                                                    <Save size={14} />
                                                </button>
                                                <button
                                                    onClick={() => setEditingPriority(null)}
                                                    className="p-1 text-muted-foreground hover:text-primary/80 transition-colors"
                                                    title="Cancel"
                                                >
                                                    <X size={14} />
                                                </button>
                                            </div>
                                        ) : (
                                            <span>Priority: {plugin.priority}</span>
                                        )}
                                    </div>
                                </div>
                            </div>
                            <div className="flex gap-2 md:opacity-0 md:group-hover:opacity-100 transition-opacity">
                                {!editingPriority && (
                                    <>
                                        <button
                                            onClick={() => setEditingPriority({ id: plugin.id, priority: plugin.priority })}
                                            className="p-2 text-muted-foreground hover:text-primary rounded-md hover:bg-card transition-colors"
                                            title="Edit Priority"
                                        >
                                            <Sliders size={16} />
                                        </button>
                                        <button
                                            onClick={() => handleResetPriority(plugin.id)}
                                            className="p-2 text-muted-foreground hover:text-primary rounded-md hover:bg-card transition-colors"
                                            title="Reset Priority"
                                        >
                                            <Sliders size={16} className="rotate-90" />
                                        </button>
                                        <button
                                            onClick={() => handleUninstallPlugin(plugin.id)}
                                            className="p-2 text-muted-foreground hover:text-red-400 rounded-md hover:bg-card transition-colors"
                                            title="Uninstall Plugin"
                                        >
                                            <Trash2 size={16} />
                                        </button>
                                    </>
                                )}
                            </div>
                        </div>
                    ))}
                    {(!installedPlugins || installedPlugins.length === 0) && (
                        <div className="text-center py-6 text-muted-foreground">
                            <p className="text-sm">No plugins installed</p>
                        </div>
                    )}
                </div>
            </div>

            {/* Available Plugins */}
            <div>
                <h3 className="text-sm font-medium text-muted-foreground mb-3">Available Plugins</h3>
                <div className="space-y-3">
                    {availablePlugins?.map((plugin: Plugin) => {
                        const isInstalled = installedPlugins?.some(
                            (p:PluginInstall) => p.name === plugin.name && p.version === plugin.version
                        );
                        return (
                            <div key={`${plugin.name}-${plugin.version}`}
                                className="flex items-center justify-between p-3 bg-card/50 rounded-lg hover:bg-card/80 transition-colors">
                                <div className="flex items-center gap-3">
                                    <Puzzle size={16} className="text-primary" />
                                    <div>
                                        <div className="font-medium text-sm">{plugin.name}</div>
                                        <div className="text-xs text-muted-foreground">Version {plugin.version}</div>
                                    </div>
                                </div>
                                <button
                                    onClick={() => handleInstallPlugin(plugin)}
                                    disabled={isInstalled}
                                    className="px-3 py-1 text-xs bg-primary text-white rounded-md hover:bg-blue-600 
                                    transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                >
                                    {isInstalled ? "Installed" : "Install"}
                                </button>
                            </div>
                        );
                    })}
                </div>
            </div>
        </div>
    );
};