import { FileJson, Globe, Loader2, Plus, Upload, X } from "lucide-react";
import {
  useCreateApiDefinition,
  useImportOpenApiDefinition,
} from "../../api/api-definitions";

import { displayError } from "../../lib/error-utils";
import toast from "react-hot-toast";
import { useState } from "react";

interface ApiDefinitionModalProps {
  isOpen: boolean;
  onClose: () => void;
  onApiDefinitionCreated: (apiDefinitionId: string) => void;
}

type CreationMethod = "manual" | "upload";

const TabButton = ({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) => (
  <button
    onClick={onClick}
    className={`flex items-center gap-2 px-4 py-2 rounded-lg transition-colors 
                   ${
                     active
                       ? "bg-primary/10 text-primary"
                       : "text-muted-foreground hover:text-gray-300 hover:bg-card/50"
                   }`}
  >
    {children}
  </button>
);

export const ApiDefinitionModal = ({
  isOpen,
  onClose,
  onApiDefinitionCreated,
}: ApiDefinitionModalProps) => {
  const [creationMethod, setCreationMethod] =
    useState<CreationMethod>("manual");
  const [name, setName] = useState("");
  const [version, setVersion] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [dragActive, setDragActive] = useState(false);

  const createDefinition = useCreateApiDefinition();
  const importDefinition = useImportOpenApiDefinition();

  const handleSubmit = async () => {
    if ((!name || !version) && !file) return;

    setIsSubmitting(true);

    try {
      if (creationMethod === "manual") {
        await handleManualCreation();
      } else {
        if (!file) {
          toast.error("File is required for import");
          return;
        }
        await handleFileImport(file);
      }
    } catch (error) {
      displayError(error, "An error occurred during submission");
      console.error(error);
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleManualCreation = async () => {
    const apiDefinition = {
      id: name,
      version,
      draft: true,
      routes: [],
    };

    try {
      const createdDefinition =
        await createDefinition.mutateAsync(apiDefinition);
      toast.success("API definition created successfully");
      onApiDefinitionCreated(createdDefinition.id);
    } catch (error) {
      displayError(error, "Failed to create API definition");
      console.error(error);
    }
  };

  const handleFileImport = (file: File) => {
    return new Promise<void>((resolve, reject) => {
      const reader = new FileReader();

      reader.onload = async (e) => {
        try {
          const spec = e.target?.result as string;
          const openApiDoc = JSON.parse(spec);

          validateOpenApiDoc(openApiDoc);

          const createdDefinition =
            await importDefinition.mutateAsync(openApiDoc);
          toast.success("API definition imported successfully");
          onApiDefinitionCreated(createdDefinition.id);

          resetForm();
          onClose();
          resolve();
        } catch (error) {
          displayError(error, "Failed to import API definition");
          console.error(error);
          reject(error);
        }
      };

      reader.onerror = () => {
        toast.error("Failed to read file");
        reject(new Error("File reading error"));
      };

      reader.readAsText(file);
    });
  };

  const validateOpenApiDoc = (doc: Record<string, string>) => {
    if (!doc.openapi || !doc.info || !doc.paths) {
      throw new Error("Invalid OpenAPI specification: Missing required fields");
    }

    if (
      !doc["x-golem-api-definition-id"] ||
      !doc["x-golem-api-definition-version"]
    ) {
      throw new Error("Missing required Golem API definition fields");
    }
  };

  const resetForm = () => {
    setName("");
    setVersion("");
    setFile(null);
    setCreationMethod("manual");
  };

  const handleFileDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragActive(false);
    const droppedFile = e.dataTransfer.files[0];
    if (
      droppedFile?.name.endsWith(".json") ||
      droppedFile?.name.endsWith(".openapi.json")
    ) {
      setFile(droppedFile);
    } else {
      toast.error("Please upload an OpenAPI specification JSON file");
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
      <div className="bg-card rounded-xl p-6 max-w-md w-full shadow-xl">
        <div className="flex justify-between items-start mb-6">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-md bg-primary/10 text-primary">
              <Globe size={24} />
            </div>
            <div>
              <h2 className="text-xl font-semibold">Create API Definition</h2>
              <p className="text-sm text-muted-foreground mt-1">
                Define your API endpoints
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-gray-300 p-1 hover:bg-card/50 
                                 rounded-md transition-colors"
          >
            <X size={20} />
          </button>
        </div>

        <div className="flex gap-2 mb-6">
          <TabButton
            active={creationMethod === "manual"}
            onClick={() => setCreationMethod("manual")}
          >
            <Plus size={18} />
            Create Manually
          </TabButton>
          <TabButton
            active={creationMethod === "upload"}
            onClick={() => setCreationMethod("upload")}
          >
            <Upload size={18} />
            Upload Spec
          </TabButton>
        </div>

        <div className="space-y-6">
          {creationMethod === "manual" ? (
            <>
              <div>
                <label className="block text-sm font-medium mb-1.5 text-gray-300">
                  Name
                </label>
                <input
                  type="text"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  className="w-full px-4 py-2.5 bg-card/50 rounded-lg border border-gray-600 
                                             focus:border-blue-500 focus:ring-1 focus:ring-blue-500 outline-none"
                  placeholder="Enter API name"
                  disabled={isSubmitting}
                />
              </div>
              <div>
                <label className="block text-sm font-medium mb-1.5 text-gray-300">
                  Version
                </label>
                <input
                  type="text"
                  value={version}
                  onChange={(e) => setVersion(e.target.value)}
                  className="w-full px-4 py-2.5 bg-card/50 rounded-lg border border-gray-600 
                                             focus:border-blue-500 focus:ring-1 focus:ring-blue-500 outline-none"
                  placeholder="e.g., 1.0.0"
                  disabled={isSubmitting}
                />
              </div>
            </>
          ) : (
            <div
              onDragOver={(e) => {
                e.preventDefault();
                if (!isSubmitting) {
                  setDragActive(true);
                }
              }}
              onDragLeave={() => setDragActive(false)}
              onDrop={handleFileDrop}
              className={`border-2 border-dashed rounded-lg p-8 text-center transition-all
                                ${isSubmitting ? "cursor-not-allowed opacity-60" : "cursor-pointer"} 
                                ${dragActive ? "border-blue-500 bg-primary/10" : "border-gray-600"}`}
            >
              {file ? (
                <div className="flex items-center justify-center gap-3">
                  <FileJson className="h-6 w-6 text-primary" />
                  <span>{file.name}</span>
                  {!isSubmitting && (
                    <button
                      onClick={() => setFile(null)}
                      className="p-1 text-muted-foreground hover:text-red-400 rounded-md
                     hover:bg-red-500/10 transition-colors"
                    >
                      <X size={16} />
                    </button>
                  )}
                </div>
              ) : (
                <div
                  className="space-y-2"
                  onClick={() =>
                    document.getElementById("file-upload")?.click()
                  }
                >
                  <Upload className="h-8 w-8 mx-auto text-muted-foreground" />
                  <div>
                    <p className="text-sm text-gray-300">
                      Upload your OpenAPI specification
                    </p>
                    <p className="text-xs text-muted-foreground mt-1">
                      Drag and drop or click to browse
                    </p>
                  </div>
                </div>
              )}
              <input
                type="file"
                accept=".json,.yaml"
                onChange={(e) => setFile(e.target.files?.[0] || null)}
                className="hidden"
                disabled={isSubmitting}
                id="file-upload"
              />
            </div>
          )}

          <div className="flex justify-end items-center gap-3 pt-2">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm bg-card/80 rounded-lg hover:bg-gray-600 
                                     transition-colors disabled:opacity-50"
              disabled={isSubmitting}
            >
              Cancel
            </button>
            <button
              onClick={handleSubmit}
              disabled={
                (creationMethod === "manual" && (!name || !version)) ||
                (creationMethod === "upload" && !file) ||
                isSubmitting
              }
              className="px-4 py-2 text-sm bg-primary rounded-lg hover:bg-blue-600 
                                     disabled:opacity-50 transition-colors flex items-center gap-2"
            >
              {isSubmitting ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  <span>Creating...</span>
                </>
              ) : (
                <>
                  <Plus size={16} />
                  <span>Create Definition</span>
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
