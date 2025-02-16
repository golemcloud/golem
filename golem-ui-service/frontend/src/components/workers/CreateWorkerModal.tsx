import {
  AlertCircle,
  Key,
  Loader2,
  Plus,
  Settings,
  Terminal,
  X,
} from "lucide-react";

import toast from "react-hot-toast";
import { type InputProps } from "../components/CreateComponentModal";
import { useCreateWorker } from "../../api/workers";
import { useState } from "react";
import { UseMutationResult } from "@tanstack/react-query";
import { GolemError } from "../../types/error";

const Input: React.FC<InputProps> = ({ label, error, ...props }) => (
  <div>
    <label className="block text-sm font-medium mb-1.5 text-gray-300">
      {label}
    </label>
    <input
      {...props}
      className="w-full px-4 py-2.5 bg-card/50 rounded-lg border border-gray-600 
                     focus:border-blue-500 focus:ring-1 focus:ring-blue-500 outline-none
                     transition-all duration-200 disabled:opacity-50 disabled:cursor-not-allowed"
    />
    {error && (
      <div className="mt-1 flex items-center gap-1 text-red-400 text-sm">
        <AlertCircle size={14} />
        <span>{error}</span>
      </div>
    )}
  </div>
);

export const CreateWorkerModal = ({
  isOpen,
  onClose,
  componentId,
}: {
  isOpen: boolean;
  onClose: () => void;
  componentId: string;
}) => {
  const [name, setName] = useState("");
  const [env, setEnv] = useState<{ key: string; value: string }[]>([
    { key: "", value: "" },
  ]);
  const [args, setArguments] = useState<string[]>([]);

  const createWorker: UseMutationResult<
    Worker,
    GolemError,
    {
      name: string;
      env: Record<string, string>;
      args: string[];
    }
  > = useCreateWorker(componentId);

  const handleSubmit = () => {
    const envRecord = env.reduce(
      (acc, { key, value }) => {
        if (key) acc[key] = value;
        return acc;
      },
      {} as Record<string, string>,
    );

    createWorker.mutate(
      {
        name: name.replace(/ /g, "-"),
        env: envRecord,
        args,
      },
      {
        onSuccess: () => {
          toast.success("Worker created successfully");
          onClose();
        },
      },
    );
  };

  const removeEnvVar = (index: number) => {
    setEnv(env.filter((_, i) => i !== index));
  };

  const removeArg = (index: number) => {
    setArguments(args.filter((_, i) => i !== index));
  };

  if (!isOpen) return null;

  return (
    <div className="-top-8 fixed inset-0 bg-black/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
      <div className="bg-card rounded-xl shadow-xl w-full max-w-2xl">
        {/* Header */}
        <div className="p-6 border-b border-gray-700">
          <div className="flex justify-between items-start">
            <div className="flex items-center gap-3">
              <div className="p-2 rounded-md bg-primary/10 text-primary">
                <Terminal size={20} />
              </div>
              <div>
                <h2 className="text-xl font-semibold">Create New Worker</h2>
                <p className="text-sm text-muted-foreground mt-1">
                  Configure worker settings
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
        </div>

        {/* Scrollable Content */}
        <div className="p-6 max-h-[calc(100vh-16rem)] overflow-y-auto">
          <div className="space-y-6">
            <Input
              label="Worker Name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Enter worker name"
              disabled={createWorker.isPending}
            />

            {/* Environment Variables */}
            <div>
              <div className="flex justify-between items-center mb-2">
                <label className="block text-sm font-medium text-gray-300">
                  Environment Variables
                </label>
                <button
                  onClick={() => setEnv([...env, { key: "", value: "" }])}
                  className="text-sm text-primary hover:text-primary-accent flex items-center gap-1
                                             px-2 py-1 rounded hover:bg-primary/10 transition-colors"
                  disabled={createWorker.isPending}
                >
                  <Plus size={14} />
                  Add Variable
                </button>
              </div>
              <div className="space-y-2 max-h-64 overflow-y-auto pr-1">
                {env.map((item, index) => (
                  <div
                    key={index}
                    className="flex gap-2 items-center p-2 rounded-lg bg-gray-700/30 
                                                  group hover:bg-card/50 transition-colors"
                  >
                    <Key
                      size={16}
                      className="text-muted-foreground flex-shrink-0"
                    />
                    <input
                      placeholder="Key"
                      value={item.key}
                      onChange={(e) => {
                        const newEnv = [...env];
                        newEnv[index].key = e.target.value;
                        setEnv(newEnv);
                      }}
                      className="flex-1 min-w-0 px-3 py-1.5 bg-card/50 rounded-md border border-gray-600
                                                     focus:border-blue-500 outline-none transition-colors"
                      disabled={createWorker.isPending}
                    />
                    <input
                      placeholder="Value"
                      value={item.value}
                      onChange={(e) => {
                        const newEnv = [...env];
                        newEnv[index].value = e.target.value;
                        setEnv(newEnv);
                      }}
                      className="flex-1 min-w-0 px-3 py-1.5 bg-card/50 rounded-md border border-gray-600
                                                     focus:border-blue-500 outline-none transition-colors"
                      disabled={createWorker.isPending}
                    />
                    <button
                      onClick={() => removeEnvVar(index)}
                      className="p-1.5 text-muted-foreground hover:text-red-400 rounded-md flex-shrink-0
                                                     opacity-0 group-hover:opacity-100 transition-all hover:bg-gray-600/50"
                      disabled={createWorker.isPending}
                    >
                      <X size={14} />
                    </button>
                  </div>
                ))}
              </div>
            </div>

            {/* Arguments */}
            <div>
              <div className="flex justify-between items-center mb-2">
                <label className="block text-sm font-medium text-gray-300">
                  Arguments
                </label>
                <button
                  onClick={() => setArguments([...args, ""])}
                  className="text-sm text-primary hover:text-primary-accent flex items-center gap-1
                                             px-2 py-1 rounded hover:bg-primary/10 transition-colors"
                  disabled={createWorker.isPending}
                >
                  <Plus size={14} />
                  Add Argument
                </button>
              </div>
              <div className="space-y-2 max-h-48 overflow-y-auto pr-1">
                {args.map((arg, index) => (
                  <div
                    key={index}
                    className="flex items-center gap-2 p-2 rounded-lg bg-gray-700/30
                                                  group hover:bg-card/50 transition-colors"
                  >
                    <Settings
                      size={16}
                      className="text-muted-foreground flex-shrink-0"
                    />
                    <input
                      value={arg}
                      onChange={(e) => {
                        const newArgs = [...args];
                        newArgs[index] = e.target.value;
                        setArguments(newArgs);
                      }}
                      className="flex-1 min-w-0 px-3 py-1.5 bg-card/50 rounded-md border border-gray-600
                                                     focus:border-blue-500 outline-none transition-colors"
                      placeholder="Enter argument"
                      disabled={createWorker.isPending}
                    />
                    <button
                      onClick={() => removeArg(index)}
                      className="p-1.5 text-muted-foreground hover:text-red-400 rounded-md flex-shrink-0
                                                     opacity-0 group-hover:opacity-100 transition-all hover:bg-gray-600/50"
                      disabled={createWorker.isPending}
                    >
                      <X size={14} />
                    </button>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </div>

        {/* Footer */}
        <div className="p-6 border-t border-gray-700">
          <div className="flex justify-end items-center gap-3">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm bg-card/80 rounded-lg hover:bg-gray-600 
                                     transition-colors disabled:opacity-50"
              disabled={createWorker.isPending}
            >
              Cancel
            </button>
            <button
              onClick={handleSubmit}
              disabled={!name || createWorker.isPending}
              className="px-4 py-2 text-sm bg-primary rounded-lg hover:bg-blue-600 
                                     disabled:opacity-50 transition-colors flex items-center gap-2"
            >
              {createWorker.isPending ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  <span>Creating...</span>
                </>
              ) : (
                <>
                  <Plus size={16} />
                  <span>Create Worker</span>
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
