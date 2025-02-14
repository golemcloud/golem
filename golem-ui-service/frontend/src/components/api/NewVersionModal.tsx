import { Loader2, Tags, X } from "lucide-react";

import toast from "react-hot-toast";
import { useCreateApiDefinition } from "../../api/api-definitions";
import { useNavigate } from "react-router-dom";
import { useState } from "react";

interface VersionModalProps {
  isOpen: boolean;
  onClose: () => void;
  currentDefinition: {
    id: string;
    routes: any[];
    draft: boolean;
  };
}

const NewVersionModal = ({ isOpen, onClose, currentDefinition }: VersionModalProps) => {
  const [version, setVersion] = useState("");
  const createDefinition = useCreateApiDefinition();
  const navigate = useNavigate();
  const [isSubmitting, setIsSubmitting] = useState(false);

  const handleSubmit = async () => {
    if (!version) return;

    setIsSubmitting(true);
    try {
      const newDefinition = {
        id: currentDefinition.id,
        version,
        routes: currentDefinition.routes,
        draft: true
      };

      const result = await createDefinition.mutateAsync(newDefinition);
      toast.success("New version created successfully");
      navigate(`/apis/definitions/${result.id}/${result.version}`);
      onClose();
    } catch (error) {
      console.error(error);
    } finally {
      setIsSubmitting(false);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-background/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
      <div className="bg-card rounded-xl p-6 max-w-md w-full shadow-xl border border-border">
        <div className="flex justify-between items-start mb-6">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-md bg-primary/10 text-primary">
              <Tags size={20} />
            </div>
            <div>
              <h2 className="text-xl font-semibold">Create New Version</h2>
              <p className="text-sm text-muted-foreground mt-1">
                Create a new version of {currentDefinition.id}
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-foreground p-1 hover:bg-muted/50 rounded-md transition-colors"
          >
            <X size={20} />
          </button>
        </div>

        <div className="space-y-4">
          <div>
            <label className="block text-sm font-medium mb-1.5">Version Number</label>
            <input
              type="text"
              value={version}
              onChange={(e) => setVersion(e.target.value)}
              placeholder="e.g., 2.0.0"
              className="w-full px-4 py-2.5 bg-card/50 rounded-lg border border-input
                focus:border-primary focus:ring-1 focus:ring-primary outline-none"
              disabled={isSubmitting}
            />
          </div>

          <div className="bg-muted/50 rounded-lg p-4 text-sm text-muted-foreground">
            <p>This will create a new draft version with the same routes as the current version.</p>
          </div>

          <div className="flex justify-end gap-3 pt-4">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm bg-muted hover:bg-muted/80 rounded-lg transition-colors"
              disabled={isSubmitting}
            >
              Cancel
            </button>
            <button
              onClick={handleSubmit}
              disabled={!version || isSubmitting}
              className="px-4 py-2 text-sm bg-primary text-primary-foreground rounded-lg
                hover:bg-primary/90 disabled:opacity-50 transition-colors flex items-center gap-2"
            >
              {isSubmitting ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  <span>Creating...</span>
                </>
              ) : (
                <>
                  <Tags size={16} />
                  <span>Create Version</span>
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

export default NewVersionModal;