import { FileJson, Globe, Loader2, Plus, Upload, X } from "lucide-react";

import toast from "react-hot-toast";
import { useCreateApiDefinition } from "../../api/api-definitions";
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
                       ? "bg-blue-500/10 text-blue-400"
                       : "text-gray-400 hover:text-gray-300 hover:bg-gray-700/50"
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

  const handleSubmit = async () => {
    if ((!name || !version) && !file) return;

    setIsSubmitting(true);
    try {
      if (creationMethod === "manual") {
        const apiDefinition = {
          id: name,
          version,
          draft: true,
          routes: [],
        };
        const createdDefinition =
          await createDefinition.mutateAsync(apiDefinition);
        toast.success("API definition created successfully");
        onApiDefinitionCreated(createdDefinition.id);
      } else {
        return;
        if (!file) return;
        const reader = new FileReader();
        reader.onload = async (e) => {
          try {
            const spec = e.target?.result as string;
            const apiDefinition = {
              id: file.name.replace(/\.[^/.]+$/, ""),
              version: "1",
              draft: true,
              routes: [],
              spec,
            };
            const createdDefinition =
              await createDefinition.mutateAsync(apiDefinition);
            toast.success("API definition uploaded successfully");
            onApiDefinitionCreated(createdDefinition.id);
          } catch (error) {
            toast.error("Failed to parse API definition");
            console.error(error);
          }
        };
        reader.readAsText(file!);
      }
      resetForm();
      onClose();
    } catch (err) {
      toast.error("Failed to create API definition");
      console.log(err);
    } finally {
      setIsSubmitting(false);
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
      droppedFile?.name.endsWith(".yaml")
    ) {
      setFile(droppedFile);
    } else {
      toast.error("Please upload a JSON or YAML file");
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
      <div className="bg-gray-800 rounded-xl p-6 max-w-md w-full shadow-xl">
        <div className="flex justify-between items-start mb-6">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-md bg-blue-500/10 text-blue-400">
              <Globe size={24} />
            </div>
            <div>
              <h2 className="text-xl font-semibold">Create API Definition</h2>
              <p className="text-sm text-gray-400 mt-1">
                Define your API endpoints
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-300 p-1 hover:bg-gray-700/50 
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
                  className="w-full px-4 py-2.5 bg-gray-700/50 rounded-lg border border-gray-600 
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
                  className="w-full px-4 py-2.5 bg-gray-700/50 rounded-lg border border-gray-600 
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
                                ${dragActive ? "border-blue-500 bg-blue-500/10" : "border-gray-600"}`}
            >
              {file ? (
                <div className="flex items-center justify-center gap-3">
                  <FileJson className="h-6 w-6 text-blue-400" />
                  <span>{file.name}</span>
                  {!isSubmitting && (
                    <button
                      onClick={() => setFile(null)}
                      className="p-1 text-gray-400 hover:text-red-400 rounded-md
                                                     hover:bg-red-500/10 transition-colors"
                    >
                      <X size={16} />
                    </button>
                  )}
                </div>
              ) : (
                <div className="space-y-2">
                  <Upload className="h-8 w-8 mx-auto text-gray-400" />
                  <div>
                    <p className="text-sm text-gray-300">
                      Upload your OpenAPI specification
                    </p>
                    <p className="text-xs text-gray-400 mt-1">
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
              />
            </div>
          )}

          <div className="flex justify-end items-center gap-3 pt-2">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm bg-gray-700 rounded-lg hover:bg-gray-600 
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
              className="px-4 py-2 text-sm bg-blue-500 rounded-lg hover:bg-blue-600 
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
