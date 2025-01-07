import { AlertCircle, Loader2, Plus, Server, Settings, X } from "lucide-react";

import { displayError } from "../../lib/error-utils";
import toast from "react-hot-toast";
import { useComponents } from "../../api/components";
import { useCreatePlugin } from "../../api/plugins";
import { useState } from "react";

type PluginType = "OplogProcessor" | "ComponentTransformer";

interface CreatePluginModalProps {
  isOpen: boolean;
  onClose: () => void;
}

interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label: string;
  error?: string;
}

const Input: React.FC<InputProps> = ({ label, error, ...props }) => (
  <div>
    <label className="block text-sm font-medium mb-1.5 text-gray-300">
      {label}
    </label>
    <input
      {...props}
      className="w-full px-3 md:px-4 py-2 md:py-2.5 bg-card/50 rounded-lg border border-gray-600 
               focus:border-blue-500 focus:ring-1 focus:ring-blue-500 outline-none transition duration-200
               disabled:opacity-50 disabled:cursor-not-allowed text-sm md:text-base"
    />
    {error && (
      <div className="mt-1 flex items-center gap-1 text-red-400 text-xs md:text-sm">
        <AlertCircle size={14} className="flex-shrink-0" />
        <span>{error}</span>
      </div>
    )}
  </div>
);

export const CreatePluginModal = ({ isOpen, onClose }: CreatePluginModalProps) => {
  const [name, setName] = useState("");
  const [version, setVersion] = useState("");
  const [description, setDescription] = useState("");
  const [homepage, setHomepage] = useState("");
  const [type, setType] = useState<PluginType>("ComponentTransformer");
  const [isSubmitting, setIsSubmitting] = useState(false);

  // OplogProcessor fields
  const [selectedComponentId, setSelectedComponentId] = useState("");
  const [selectedVersion, setSelectedVersion] = useState<number>(0);

  // ComponentTransformer fields
  const [jsonSchema, setJsonSchema] = useState("");
  const [validateUrl, setValidateUrl] = useState("");
  const [transformUrl, setTransformUrl] = useState("");

  const { data: components } = useComponents();
  const createPlugin = useCreatePlugin();

  const handleSubmit = async () => {
    setIsSubmitting(true);

    const pluginData = {
      name,
      version,
      description,
      specs:
        type === "OplogProcessor"
          ? {
              type: "OplogProcessor",
              componentId: selectedComponentId,
              componentVersion: selectedVersion,
            }
          : {
              type: "ComponentTransformer",
              jsonSchema,
              validateUrl,
              transformUrl,
            },
      scope: {
        type: "Global",
      },
      icon: [0],
      homepage,
    };

    try {
      await createPlugin.mutateAsync(pluginData);
      toast.success("Plugin created successfully");
      resetForm();
      onClose();
    } catch (error) {
      console.error(error);
    } finally {
      setIsSubmitting(false);
    }
  };

  const resetForm = () => {
    setName("");
    setVersion("");
    setDescription("");
    setType("ComponentTransformer");
    setSelectedComponentId("");
    setSelectedVersion(0);
    setJsonSchema("");
    setValidateUrl("");
    setTransformUrl("");
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/60 flex items-start md:items-center justify-center p-4 z-50 backdrop-blur-sm overflow-y-auto">
      <div className="bg-card rounded-xl p-4 md:p-6 w-full max-w-2xl shadow-xl my-4 md:my-0">
        <div className="flex justify-between items-start mb-4 md:mb-6">
          <div className="flex items-center gap-2 md:gap-3">
            <div className="p-2 rounded-md bg-primary/10 text-primary">
              <Plus size={20} />
            </div>
            <div>
              <h2 className="text-lg md:text-xl font-semibold">Create New Plugin</h2>
              <p className="text-xs md:text-sm text-muted-foreground mt-1">
                Configure your plugin settings
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-gray-300 p-1 hover:bg-card/50 rounded-md transition-colors"
            aria-label="Close modal"
          >
            <X size={20} />
          </button>
        </div>

        <div className="space-y-4 md:space-y-6">
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
            <Input
              label="Plugin Name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              disabled={isSubmitting}
              placeholder="Enter plugin name"
            />
            <Input
              label="Version"
              value={version}
              onChange={(e) => setVersion(e.target.value)}
              disabled={isSubmitting}
              placeholder="e.g., 1.0.0"
            />
          </div>

          <Input
            label="Description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            disabled={isSubmitting}
            placeholder="Brief description of your plugin"
          />

          <Input
            label="Homepage"
            value={homepage}
            onChange={(e) => setHomepage(e.target.value)}
            disabled={isSubmitting}
            placeholder="https://"
          />

          <div>
            <label className="block text-sm font-medium mb-1.5 text-gray-300">
              Plugin Type
            </label>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 md:gap-4">
              {[
                {
                  value: "OplogProcessor",
                  label: "Oplog Processor",
                  icon: Server,
                },
                {
                  value: "ComponentTransformer",
                  label: "Component Transformer",
                  icon: Settings,
                },
              ].map((option) => (
                <button
                  key={option.value}
                  onClick={() => setType(option.value as PluginType)}
                  className={`flex items-center gap-3 p-3 md:p-4 rounded-lg border-2 transition-all
                           ${type === option.value
                              ? "border-blue-500 bg-primary/10"
                              : "border-gray-600 hover:border-gray-500"
                           }`}
                  disabled={isSubmitting}
                >
                  <option.icon
                    className={type === option.value ? "text-primary" : "text-muted-foreground"}
                    size={20}
                  />
                  <span className="text-sm md:text-base">{option.label}</span>
                </button>
              ))}
            </div>
          </div>

          {type === "OplogProcessor" ? (
            <div className="space-y-4 border-t border-gray-700 pt-4">
              <div>
                <label className="block text-sm font-medium mb-1.5 text-gray-300">
                  Component
                </label>
                <select
                  value={selectedComponentId}
                  onChange={(e) => setSelectedComponentId(e.target.value)}
                  className="w-full px-3 md:px-4 py-2 md:py-2.5 bg-card/50 rounded-lg border border-gray-600 
                           focus:border-blue-500 outline-none text-sm md:text-base"
                  disabled={isSubmitting}
                >
                  <option value="">Select a component</option>
                  {components?.map((component) => (
                    <option
                      key={component.versionedComponentId.componentId}
                      value={component.versionedComponentId.componentId}
                    >
                      {component.componentName}
                    </option>
                  ))}
                </select>
              </div>

              {selectedComponentId && (
                <Input
                  label="Version"
                  type="number"
                  value={selectedVersion}
                  onChange={(e) => setSelectedVersion(Number(e.target.value))}
                  disabled={isSubmitting}
                  min="0"
                />
              )}
            </div>
          ) : (
            <div className="space-y-4 border-t border-gray-700 pt-4">
              <div>
                <label className="block text-sm font-medium mb-1.5 text-gray-300">
                  JSON Schema
                </label>
                <textarea
                  value={jsonSchema}
                  onChange={(e) => setJsonSchema(e.target.value)}
                  className="w-full px-3 md:px-4 py-2 md:py-2.5 bg-card/50 rounded-lg border border-gray-600 
                           focus:border-blue-500 outline-none font-mono text-xs md:text-sm h-24 md:h-32 resize-none"
                  placeholder="{}"
                  disabled={isSubmitting}
                />
              </div>
              <Input
                label="Validate URL"
                type="url"
                value={validateUrl}
                onChange={(e) => setValidateUrl(e.target.value)}
                disabled={isSubmitting}
                placeholder="https://"
              />
              <Input
                label="Transform URL"
                type="url"
                value={transformUrl}
                onChange={(e) => setTransformUrl(e.target.value)}
                disabled={isSubmitting}
                placeholder="https://"
              />
            </div>
          )}

          <div className="flex flex-col-reverse sm:flex-row sm:justify-end items-stretch sm:items-center gap-3 pt-2">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm bg-card/80 rounded-lg hover:bg-gray-600 transition-colors
                       disabled:opacity-50 w-full sm:w-auto"
              disabled={isSubmitting}
            >
              Cancel
            </button>
            <button
              onClick={handleSubmit}
              disabled={!name || !version || isSubmitting}
              className="px-4 py-2 text-sm bg-primary rounded-lg hover:bg-blue-600 disabled:opacity-50
                       transition-colors flex items-center justify-center gap-2 w-full sm:w-auto"
            >
              {isSubmitting ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  <span>Creating...</span>
                </>
              ) : (
                <>
                  <Plus size={16} />
                  <span>Create Plugin</span>
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};