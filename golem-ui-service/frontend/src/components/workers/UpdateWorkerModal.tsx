import { AlertCircle, PauseCircle, PlayCircle, XCircle } from "lucide-react";
import { useInterruptWorker, useResumeWorker } from "../../api/workers";

import toast from "react-hot-toast";
import { useState } from "react";

// import { useWorkers } from '../api/workers';

interface WorkerActionModalProps {
  isOpen: boolean;
  onClose: () => void;
  workerId: {
    componentId: string;
    workerName: string;
  };
  action: "interrupt" | "resume";
  currentStatus: string;
}

export const WorkerActionModal = ({
  isOpen,
  onClose,
  workerId,
  action,
  currentStatus,
}: WorkerActionModalProps) => {
  const [recoverImmediately, setRecoverImmediately] = useState(false);
  const { componentId, workerName } = workerId;

  const { mutate: interruptWorker } = useInterruptWorker({
    onSuccess: () => {
      toast.success("Worker interrupted successfully");
      onClose();
    },
    onError: (error) => {
      toast.error(`Failed to interrupt worker: ${error.message}`);
    },
  });

  const { mutate: resumeWorker } = useResumeWorker({
    onSuccess: () => {
      toast.success("Worker resumed successfully");
      onClose();
    },
    onError: (error) => {
      toast.error(`Failed to resume worker: ${error.message}`);
    },
  });

  const handleAction = () => {
    if (action === "interrupt") {
      interruptWorker({ componentId, workerName, recoverImmediately });
    } else {
      resumeWorker({ componentId, workerName });
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4">
      <div className="bg-card rounded-lg p-6 max-w-md w-full">
        <div className="flex justify-between items-start mb-4">
          <div className="flex items-center gap-2">
            {action === "interrupt" ? (
              <PauseCircle className="h-6 w-6 text-yellow-500" />
            ) : (
              <PlayCircle className="h-6 w-6 text-green-500" />
            )}
            <h2 className="text-xl font-semibold">
              {action === "interrupt" ? "Interrupt Worker" : "Resume Worker"}
            </h2>
          </div>
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-gray-300"
          >
            <XCircle className="h-6 w-6" />
          </button>
        </div>

        <div className="space-y-4">
          <div>
            <p className="text-gray-300">
              Worker: <span className="font-medium">{workerName}</span>
            </p>
            <p className="text-muted-foreground text-sm">
              Current Status:{" "}
              <span className="font-medium">{currentStatus}</span>
            </p>
          </div>

          {action === "interrupt" && (
            <div className="flex items-center gap-2">
              <input
                type="checkbox"
                id="recover-immediately"
                checked={recoverImmediately}
                onChange={(e) => setRecoverImmediately(e.target.checked)}
                className="rounded bg-card/80 border-gray-600 text-blue-500 focus:ring-blue-500"
              />
              <label
                htmlFor="recover-immediately"
                className="text-sm text-gray-300"
              >
                Recover immediately after interruption
              </label>
            </div>
          )}

          <div className="bg-card/80 rounded p-3 flex items-start gap-2">
            <AlertCircle className="h-5 w-5 text-yellow-500 flex-shrink-0 mt-0.5" />
            <p className="text-sm text-gray-300">
              {action === "interrupt"
                ? "Interrupting a worker will pause its execution. The worker's status will be 'Interrupted' unless recover-immediately is selected."
                : "Resuming a worker will continue its execution from the interrupted state."}
            </p>
          </div>

          <div className="flex justify-end space-x-3 mt-6">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm bg-card/80 rounded-md hover:bg-gray-600"
            >
              Cancel
            </button>
            <button
              onClick={handleAction}
              className={`px-4 py-2 text-sm rounded-md ${
                action === "interrupt"
                  ? "bg-yellow-500 hover:bg-yellow-600"
                  : "bg-green-500 hover:bg-green-600"
              }`}
            >
              {action === "interrupt" ? "Interrupt Worker" : "Resume Worker"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
