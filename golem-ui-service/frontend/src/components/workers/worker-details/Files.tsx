import { Component, Worker } from "../../../types/api";
import {
  Download,
  FileText,
  Folder,
  Lock,
  UnlockKeyhole,
} from "lucide-react";
import React, { useEffect } from "react";
import { getComponentVersion, useComponent } from "../../../api/components";

import { displayError } from "../../../lib/error-utils";
import { downloadWorkerFile } from "../../../api/workers";
import toast from "react-hot-toast";

interface FilesTabProps {
  worker: Worker;
}

const FileRow: React.FC<{
  file: {
    path: string;
    permissions: "read-only" | "read-write";
    key: string;
  };
  onDownload: (fileName: string) => void;
}> = ({ file, onDownload }) => {
  // Extract filename from path
  const fileName = file.path.split("/").pop() || file.path;

  return (
    <div className="p-4 bg-card/60 rounded-lg hover:bg-card/70 transition-colors group">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <FileText size={16} className="text-primary" />
          <div>
            <div className="font-medium flex items-center gap-2">
              {fileName}
              {file.permissions === "read-only" ? (
                <Lock size={12} className="text-muted-foreground" />
              ) : (
                <UnlockKeyhole size={12} className="text-muted-foreground" />
              )}
            </div>
            <div className="text-sm text-muted-foreground">
              {file.permissions} • ID: {file.key.slice(0, 8)}...
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};

const FilesTab: React.FC<FilesTabProps> = ({ worker }) => {
  const [component, setComponent] = React.useState<Component>();
  useEffect(() => {
    getComponentVersion(worker.workerId.componentId!, worker.componentVersion).then(
      (component) => setComponent(component),
    );
  }, [worker.workerId.componentId, worker.componentVersion]);


  const handleDownload = async (filePath: string) => {
    try {
      const blob = await downloadWorkerFile(
        worker.workerId.componentId,
        worker.workerId.workerName,
        filePath
      );

      // Create download link
      const url = window.URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = filePath.split("/").pop() || filePath;
      document.body.appendChild(a);
      a.click();
      window.URL.revokeObjectURL(url);
      document.body.removeChild(a);

      toast.success("File download started");
    } catch (error) {
      displayError(error, "Failed to download file");
      console.error("Download failed:", error);
    }
  };

  if (!component) {
    return (
      <div className="bg-card/80 border border-border/10 rounded-lg p-6">
        <div className="text-center py-8 text-muted-foreground">
          <div className="animate-spin w-6 h-6 border-2 border-primary border-t-transparent rounded-full mx-auto mb-2" />
          <p>Loading files...</p>
        </div>
      </div>
    );
  }

  if (!component?.files || component.files.length === 0) {
    return (
      <div className="bg-card/80 border border-border/10 rounded-lg p-6">
        <div className="text-center py-8 text-muted-foreground">
          <Folder size={24} className="mx-auto mb-2 opacity-50" />
          <p>No files available</p>
        </div>
      </div>
    );
  }

  return (
    <div className="bg-card/80 border border-border/10 rounded-lg p-6">
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-lg font-semibold flex items-center gap-2">
          <Folder size={20} className="text-primary" />
          Component Files
          <span className="text-sm text-muted-foreground font-normal">
            ({component.files.length})
          </span>
        </h3>
      </div>

      <div className="space-y-2">
        {component.files.map((file) => (
          <FileRow
            key={file.key}
            file={file}
            onDownload={handleDownload}
          />
        ))}
      </div>
    </div>
  );
};

export default FilesTab;