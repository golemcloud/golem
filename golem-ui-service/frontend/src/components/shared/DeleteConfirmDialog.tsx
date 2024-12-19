import { AlertTriangle, Loader2, Trash2 } from "lucide-react";

const DeleteConfirmDialog = ({
  isOpen,
  onClose,
  onConfirm,
  pluginName,
  isDeleting,
  modelName = "Plugin",
}: {
  isOpen: boolean;
  onClose: () => void;
  onConfirm: () => void;
  pluginName: string;
  isDeleting: boolean;
  modelName: string;
}) => {
  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
      <div className="bg-gray-800 rounded-xl p-6 max-w-md w-full shadow-xl border border-red-500/10">
        <div className="flex items-start gap-4">
          <div className="p-3 rounded-full bg-red-500/10">
            <AlertTriangle className="text-red-400" size={24} />
          </div>
          <div className="flex-1">
            <h3 className="text-lg font-semibold text-red-400">
              Delete {modelName}
            </h3>
            <p className="mt-2 text-gray-300">
              Are you sure you want to delete{" "}
              <span className="font-semibold">{pluginName}</span>? This action
              cannot be undone.
            </p>

            <div className="flex justify-end gap-3 mt-6">
              <button
                onClick={onClose}
                disabled={isDeleting}
                className="px-4 py-2 text-sm bg-gray-700 rounded-lg hover:bg-gray-600 
                                         transition-colors disabled:opacity-50"
              >
                Cancel
              </button>
              <button
                onClick={onConfirm}
                disabled={isDeleting}
                className="px-4 py-2 text-sm bg-red-500 text-white rounded-lg hover:bg-red-600 
                                         transition-colors disabled:opacity-50 flex items-center gap-2"
              >
                {isDeleting ? (
                  <>
                    <Loader2 size={16} className="animate-spin" />
                    <span>Deleting...</span>
                  </>
                ) : (
                  <>
                    <Trash2 size={16} />
                    <span>Delete {modelName}</span>
                  </>
                )}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};

export default DeleteConfirmDialog;
