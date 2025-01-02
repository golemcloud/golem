import {
  AlertCircle,
  ArrowLeft,
  CheckCircle2,
  Code,
  Copy,
  ExternalLink,
  Globe,
  Loader2,
  Package,
  Puzzle,
  Server,
  Settings,
  Terminal,
} from "lucide-react";
import { useEffect, useState } from "react";

import DeleteConfirmDialog from "../components/shared/DeleteConfirmDialog";
import { Link } from "react-router-dom";
import { Trash2 } from "lucide-react";
import toast from "react-hot-toast";
import { useDeletePlugin } from "../api/plugins";
import { useNavigate } from "react-router-dom";
import { useParams } from "react-router-dom";
import { usePluginVersion } from "../api/plugins";

const JsonDisplay = ({ data }: { data: string }) => {
  const [formattedJson, setFormattedJson] = useState<string>("");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    try {
      const parsed = JSON.parse(data);
      setFormattedJson(JSON.stringify(parsed, null, 2));
    } catch (err) {
      setFormattedJson(data);
      console.error(err);
    }
  }, [data]);

  const handleCopy = async () => {
    await navigator.clipboard.writeText(formattedJson);
    setCopied(true);
    toast.success("Copied to clipboard");
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="relative">
      <pre className="bg-gray-900 p-4 rounded-lg overflow-x-auto text-sm border border-gray-800">
        <code className="text-foreground/90 font-mono whitespace-pre">
          {formattedJson}
        </code>
      </pre>
      <button
        onClick={handleCopy}
        className="absolute top-3 right-3 p-2 text-muted-foreground hover:text-gray-300 
                         bg-card/50 hover:bg-card rounded-md transition-all group"
      >
        {copied ? (
          <CheckCircle2 size={16} className="text-green-400" />
        ) : (
          <Copy size={16} />
        )}
      </button>
    </div>
  );
};

interface DetailsCardProps {
  title: string;
  icon: React.ComponentType<{ size: number }>;
  children: React.ReactNode;
}

const DetailsCard: React.FC<DetailsCardProps> = ({
  title,
  icon: Icon,
  children,
}) => (
  <div className="bg-card/50 rounded-lg p-6 border border-gray-700/50">
    <div className="flex items-center gap-3 mb-4">
      <div className="p-2 rounded-md bg-card/50 text-primary">
        <Icon size={18} />
      </div>
      <h2 className="text-lg font-semibold">{title}</h2>
    </div>
    {children}
  </div>
);

export const PluginDetailPage = () => {
  const { name, version } = useParams<{ name: string; version: string }>();
  const { data: plugin, isLoading } = usePluginVersion(name!, version!);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const deletePlugin = useDeletePlugin();
  const navigate = useNavigate();
  
  useEffect(() => {
    if (plugin) {
        document.title = `Plugins: ${name}  - Golem UI`;
    }
}, [plugin]);

  const handleDelete = async () => {
    try {
      await deletePlugin.mutateAsync({
        name: plugin!.name,
        version: plugin!.version,
      });
      toast.success("Plugin deleted successfully");
      navigate("/plugins");
    } catch (error) {
      toast.error("Failed to delete plugin");
      console.error(error);
    }
  };

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground flex items-center gap-2">
          <Loader2 className="animate-spin" size={20} />
          <span>Loading plugin details...</span>
        </div>
      </div>
    );
  }

  if (!plugin) {
    return (
      <div className="flex flex-col items-center justify-center h-64 text-muted-foreground">
        <AlertCircle size={48} className="text-gray-500 mb-4" />
        <p>Plugin not found</p>
      </div>
    );
  }

  return (
    <div className="space-y-8">
      {/* Header */}
      <div className="bg-card/50 p-6 rounded-lg border border-gray-700/50">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-4">
            <Link
              to="/plugins"
              className="p-2 text-muted-foreground hover:text-gray-300 rounded-lg 
                                     hover:bg-card/50 transition-colors"
            >
              <ArrowLeft size={20} />
            </Link>
            <div className="flex items-center gap-3">
              <div className="p-2 rounded-md bg-primary/10 text-primary">
                <Puzzle size={24} />
              </div>
              <div>
                <h1 className="text-2xl font-bold">{plugin.name}</h1>
                <div className="flex items-center gap-2 mt-1">
                  <Package size={14} className="text-muted-foreground" />
                  <span className="text-muted-foreground">
                    Version {plugin.version}
                  </span>
                </div>
              </div>
            </div>
          </div>
          <div className="flex items-center gap-3">
            <span
              className={`px-3 py-1 rounded-full text-sm
                            ${plugin.scope.type === "Global"
                  ? "bg-primary/10 text-primary"
                  : "bg-purple-500/10 text-purple-400"
                }`}
            >
              {plugin.scope.type}
            </span>
            <button
              onClick={() => setShowDeleteConfirm(true)}
              className="p-2 text-red-400 hover:text-red-300 rounded-lg 
                                     hover:bg-red-500/10 transition-colors"
              title="Delete Plugin"
            >
              <Trash2 size={20} />
            </button>
          </div>
        </div>
      </div>

      {/* Main content */}
      <div className="grid grid-cols-3 gap-6">
        {/* Info Panel */}
        <div className="col-span-1 space-y-6">
          <DetailsCard title="Plugin Details" icon={Settings}>
            <div className="space-y-4">
              <div>
                <label className="text-sm text-muted-foreground block mb-1">Type</label>
                <div className="flex items-center gap-2 text-muted-foreground/80">
                  {plugin.specs.type === "ComponentTransformer" ? (
                    <Code size={16} className="text-green-400" />
                  ) : (
                    <Server size={16} className="text-purple-400" />
                  )}
                  <span>{plugin.specs.type}</span>
                </div>
              </div>
              {plugin.homepage && (
                <div>
                  <label className="text-sm text-muted-foreground block mb-1">
                    Homepage
                  </label>
                  <a
                    href={plugin.homepage}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="flex items-center gap-2 text-primary hover:text-primary-accent 
                                                 transition-colors group"
                  >
                    <Globe size={16} />
                    <span>Visit Homepage</span>
                    <ExternalLink
                      size={14}
                      className="transition-transform group-hover:translate-x-0.5"
                    />
                  </a>
                </div>
              )}
            </div>
          </DetailsCard>

          {plugin.description && (
            <DetailsCard title="Description" icon={Terminal}>
              <p className="text-gray-300 text-sm leading-relaxed">
                {plugin.description}
              </p>
            </DetailsCard>
          )}
        </div>

        {/* Main Panel */}
        <div className="col-span-2 space-y-6">
          {plugin.specs.type === "ComponentTransformer" ? (
            <>
              <DetailsCard title="Endpoints" icon={Globe}>
                <div className="space-y-4">
                  <div>
                    <label className="text-sm text-muted-foreground block mb-1">
                      Validate URL
                    </label>
                    <div
                      className="font-mono text-sm bg-gray-900/50 p-3 rounded-lg 
                                                      border border-gray-700/50"
                    >
                      {plugin.specs.validateUrl}
                    </div>
                  </div>
                  <div>
                    <label className="text-sm text-muted-foreground block mb-1">
                      Transform URL
                    </label>
                    <div
                      className="font-mono text-sm bg-gray-900/50 p-3 rounded-lg 
                                                      border border-gray-700/50"
                    >
                      {plugin.specs.transformUrl}
                    </div>
                  </div>
                </div>
              </DetailsCard>

              {plugin.specs.jsonSchema && (
                <DetailsCard title="JSON Schema" icon={Code}>
                  <JsonDisplay data={plugin.specs.jsonSchema} />
                </DetailsCard>
              )}
            </>
          ) : (
            <DetailsCard title="Component Reference" icon={Package}>
              <div className="space-y-4">
                <div>
                  <label className="text-sm text-muted-foreground block mb-1">
                    Component ID
                  </label>
                  <div
                    className="font-mono text-sm bg-gray-900/50 p-3 rounded-lg
                                                  border border-gray-700/50"
                  >
                    {plugin.specs.componentId}
                  </div>
                </div>
                <div>
                  <label className="text-sm text-muted-foreground block mb-1">
                    Version
                  </label>
                  <div className="flex items-center gap-2">
                    <Package size={16} className="text-muted-foreground" />
                    <span>{plugin.specs.componentVersion}</span>
                  </div>
                </div>
              </div>
            </DetailsCard>
          )}
        </div>

        <DeleteConfirmDialog
          isOpen={showDeleteConfirm}
          onClose={() => setShowDeleteConfirm(false)}
          onConfirm={handleDelete}
          pluginName={plugin.name}
          isDeleting={deletePlugin.isLoading}
          modelName="Plugin"
        />
      </div>
    </div>
  );
};

export default PluginDetailPage;
