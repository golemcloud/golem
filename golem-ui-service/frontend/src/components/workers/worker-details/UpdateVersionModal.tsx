import { Loader2, RefreshCw, X } from 'lucide-react';
import React, { useState } from 'react';

import toast from 'react-hot-toast';
import { useComponentVersions } from '../../../api/components';
import { useUpdateWorkerVersion } from '../../../api/workers';

interface UpdateVersionModalProps {
    isOpen: boolean;
    onClose: () => void;
    worker: {
        workerId: {
            componentId: string;
            workerName: string;
        };
        componentVersion: number;
    };
}

export const UpdateVersionModal: React.FC<UpdateVersionModalProps> = ({
    isOpen,
    onClose,
    worker,
}) => {
    const [selectedVersion, setSelectedVersion] = useState<number>(worker.componentVersion);
    const { data: versions } = useComponentVersions(worker.workerId.componentId);
    const updateVersion = useUpdateWorkerVersion();

    const handleUpdate = async () => {
        if (selectedVersion === worker.componentVersion) {
            toast.error("Please select a different version");
            return;
        }

        try {
            await updateVersion.mutateAsync({
                componentId: worker.workerId.componentId,
                workerName: worker.workerId.workerName,
                payload: {
                    mode: "Automatic",
                    targetVersion: selectedVersion,
                },
            });
            toast.success("Worker version update initiated");
            onClose();
        } catch (error) {
            console.error(error);
        }
    };

    if (!isOpen) return null;

    return (
        <div className="-top-8 fixed inset-0 bg-background/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
            <div className="bg-card rounded-xl p-6 max-w-md w-full shadow-xl border border-border">
                <div className="flex justify-between items-start mb-6">
                    <div className="flex items-center gap-3">
                        <div className="p-2 rounded-md bg-primary/10 text-primary">
                            <RefreshCw size={20} />
                        </div>
                        <div>
                            <h2 className="text-xl font-semibold">Update Version</h2>
                            <p className="text-sm text-muted-foreground mt-1">
                                Change the component version for this worker
                            </p>
                        </div>
                    </div>
                    <button
                        onClick={onClose}
                        className="text-muted-foreground hover:text-foreground p-1 hover:bg-muted/50 
              rounded-md transition-colors"
                    >
                        <X size={20} />
                    </button>
                </div>

                <div className="space-y-6">
                    <div>
                        <label className="block text-sm font-medium mb-1.5">Select Version</label>
                        <select
                            value={selectedVersion}
                            onChange={(e) => setSelectedVersion(Number(e.target.value))}
                            className="w-full px-3 py-2 bg-card/50 rounded-lg border border-border
                focus:border-primary focus:ring-1 focus:ring-primary outline-none"
                        >
                            {versions?.map((version) => (
                                <option
                                    key={version.versionedComponentId.version}
                                    value={version.versionedComponentId.version}
                                >
                                    Version {version.versionedComponentId.version}
                                    {version.versionedComponentId.version === worker.componentVersion ?
                                        " (Current)" : ""}
                                </option>
                            ))}
                        </select>
                    </div>

                    <div className="flex justify-end gap-3 pt-2">
                        <button
                            onClick={onClose}
                            className="px-4 py-2 text-sm bg-card hover:bg-muted 
                transition-colors disabled:opacity-50 rounded-lg"
                            disabled={updateVersion.isPending}
                        >
                            Cancel
                        </button>
                        <button
                            onClick={handleUpdate}
                            disabled={selectedVersion === worker.componentVersion || updateVersion.isPending}
                            className="px-4 py-2 text-sm bg-primary text-primary-foreground rounded-lg 
                hover:bg-primary/90 disabled:opacity-50 transition-colors flex items-center gap-2"
                        >
                            {updateVersion.isPending ? (
                                <>
                                    <Loader2 size={16} className="animate-spin" />
                                    <span>Updating...</span>
                                </>
                            ) : (
                                <>
                                    <RefreshCw size={16} />
                                    <span>Update Version</span>
                                </>
                            )}
                        </button>
                    </div>
                </div>
            </div>
        </div>
    );
};