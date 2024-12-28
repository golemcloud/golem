import { ExternalLink, Globe, Server, Upload, X } from "lucide-react";

import { ApiDefinition } from "../../types/api";
// import { ApiDefinition } from '../types/api';
import toast from "react-hot-toast";
import { useCreateDeployment } from "../../api/api-definitions";
// import { useCreateDeployment } from '../api/api-deployments';
import { useState } from "react";

interface DeployModalProps {
  isOpen: boolean;
  onClose: () => void;
  apiDefinition: ApiDefinition;
}

export const DeployModal = ({
  isOpen,
  onClose,
  apiDefinition,
}: DeployModalProps) => {
  const [host, setHost] = useState("");
  const [subdomain, setSubdomain] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);

  const createDeployment = useCreateDeployment();

  const handleDeploy = async () => {
    if (!host) {
      toast.error("Host is required");
      return;
    }

    setIsSubmitting(true);
    try {
      await createDeployment.mutateAsync({
        apiDefinitions: [
          {
            id: apiDefinition.id,
            version: apiDefinition.version,
          },
        ],
        site: {
          host: host.toLowerCase().trim(),
          subdomain: subdomain.toLowerCase().trim() || undefined,
        },
      });
      toast.success("API deployed successfully");
      resetForm();
      onClose();
    } catch (error) {
      toast.error("Failed to deploy API");
      console.error(error);
    } finally {
      setIsSubmitting(false);
    }
  };

  const resetForm = () => {
    setHost("");
    setSubdomain("");
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4 z-50">
      <div className="bg-card rounded-lg p-6 max-w-md w-full">
        <div className="flex justify-between items-start mb-6">
          <div>
            <h2 className="text-xl font-semibold flex items-center gap-2">
              <Upload className="h-5 w-5 text-green-400" />
              Deploy API
            </h2>
            <p className="text-sm text-muted-foreground mt-1">
              {apiDefinition.id} v{apiDefinition.version}
            </p>
          </div>
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-gray-300"
            disabled={isSubmitting}
          >
            <X size={20} />
          </button>
        </div>

        <div className="space-y-6">
          <div className="bg-card/50 rounded-lg p-4">
            <h3 className="text-sm font-medium flex items-center gap-2 mb-2">
              <Server className="h-4 w-4 text-primary" />
              Deployment Configuration
            </h3>
            <p className="text-sm text-muted-foreground">
              Configure where your API will be deployed. The host should be a
              valid domain name where your API will be accessible.
            </p>
          </div>

          <div className="space-y-4">
            <div>
              <label className="block text-sm font-medium mb-1">
                Host <span className="text-red-400">*</span>
              </label>
              <div className="relative">
                <Globe className="absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
                <input
                  type="text"
                  value={host}
                  onChange={(e) => setHost(e.target.value)}
                  placeholder="api.example.com"
                  className="w-full pl-10 pr-3 py-2 bg-card/80 rounded-md focus:ring-2 focus:ring-blue-500"
                  disabled={isSubmitting}
                />
              </div>
            </div>

            <div>
              <label className="block text-sm font-medium mb-1">
                Subdomain <span className="text-muted-foreground">(Optional)</span>
              </label>
              <div className="relative">
                <ExternalLink className="absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
                <input
                  type="text"
                  value={subdomain}
                  onChange={(e) => setSubdomain(e.target.value)}
                  placeholder="v1"
                  className="w-full pl-10 pr-3 py-2 bg-card/80 rounded-md focus:ring-2 focus:ring-blue-500"
                  disabled={isSubmitting}
                />
              </div>
              <p className="mt-1 text-xs text-muted-foreground">
                Use subdomains to organize different versions or environments
              </p>
            </div>
          </div>

          {/* Preview */}
          {(host || subdomain) && (
            <div className="bg-gray-900 rounded-lg p-3 font-mono text-sm">
              <div className="text-muted-foreground mb-1">Preview URL:</div>
              <div className="text-green-400">
                https://{subdomain ? `${subdomain}.` : ""}
                {host}
              </div>
            </div>
          )}

          <div className="flex justify-end space-x-3 mt-6">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm bg-card/80 rounded-md hover:bg-gray-600"
              disabled={isSubmitting}
            >
              Cancel
            </button>
            <button
              onClick={handleDeploy}
              disabled={!host || isSubmitting}
              className="px-4 py-2 text-sm bg-green-500 rounded-md hover:bg-green-600 disabled:opacity-50 flex items-center gap-2"
            >
              {isSubmitting ? (
                <>
                  <svg className="animate-spin h-4 w-4" viewBox="0 0 24 24">
                    <circle
                      className="opacity-25"
                      cx="12"
                      cy="12"
                      r="10"
                      stroke="currentColor"
                      strokeWidth="4"
                      fill="none"
                    />
                    <path
                      className="opacity-75"
                      fill="currentColor"
                      d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
                    />
                  </svg>
                  Deploying...
                </>
              ) : (
                <>
                  <Upload size={16} />
                  Deploy API
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

export default DeployModal;
