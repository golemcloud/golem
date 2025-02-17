import * as Yup from "yup";

import {
  AlertCircle,
  Key,
  Loader2,
  Plus,
  Settings,
  Terminal,
  X,
} from "lucide-react";
import { Field, Form, Formik } from "formik";

import toast from "react-hot-toast";
import { useCreateWorker } from "../../api/workers";
import { useState } from "react";

interface CreateWorkerModalProps {
  isOpen: boolean;
  onClose: () => void;
  componentId: string;
}

// Simplified validation schema - only for worker name
const validationSchema = Yup.object().shape({
  name: Yup.string()
    .required("Worker name is required")
    .min(3, "Worker name must be at least 3 characters")
    .matches(
      /^[a-zA-Z0-9-_]+$/,
      "Only alphanumeric characters, hyphens, and underscores allowed"
    )
});

export const CreateWorkerModal = ({
  isOpen,
  onClose,
  componentId,
}: CreateWorkerModalProps) => {
  const [env, setEnv] = useState<{ key: string; value: string }[]>([
    { key: "", value: "" },
  ]);
  const [args, setArguments] = useState<string[]>([]);
  const createWorker = useCreateWorker(componentId);

  const handleSubmit = async (values: { name: string }) => {
    const envRecord = env.reduce(
      (acc, { key, value }) => {
        if (key) acc[key] = value;
        return acc;
      },
      {} as Record<string, string>
    );

    try {
      await createWorker.mutateAsync({
        name: values.name.replace(/ /g, "-"),
        env: envRecord,
        args,
      });
      
      toast.success("Worker created successfully");
      onClose();
    } catch (error) {
      toast.error("Failed to create worker");
    }
  };

  const removeEnvVar = (index: number) => {
    setEnv(env.filter((_, i) => i !== index));
  };

  const removeArg = (index: number) => {
    setArguments(args.filter((_, i) => i !== index));
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-background/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
      <div className="bg-card rounded-xl shadow-xl w-full max-w-2xl border border-border/10">
        <Formik
          initialValues={{ name: "" }}
          validationSchema={validationSchema}
          onSubmit={handleSubmit}
        >
          {({ errors, touched, isSubmitting }) => (
            <Form className="divide-y divide-border">
              {/* Header */}
              <div className="p-6">
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
                    type="button"
                    onClick={onClose}
                    className="text-muted-foreground hover:text-foreground p-1 hover:bg-muted/50 
                             rounded-md transition-colors"
                  >
                    <X size={20} />
                  </button>
                </div>
              </div>

              {/* Content */}
              <div className="p-6 max-h-[calc(100vh-16rem)] overflow-y-auto">
                <div className="space-y-6">
                  {/* Worker Name */}
                  <div>
                    <label className="block text-sm font-medium mb-1.5 text-foreground/80">
                      Worker Name
                    </label>
                    <Field
                      name="name"
                      className={`w-full px-4 py-2.5 bg-card/50 rounded-lg border 
                               ${errors.name && touched.name 
                                  ? 'border-destructive focus:border-destructive' 
                                  : 'border-input focus:border-primary'
                               } 
                               focus:ring-1 focus:ring-primary outline-none
                               transition-all duration-200 disabled:opacity-50`}
                      placeholder="Enter worker name"
                      disabled={isSubmitting}
                    />
                    {errors.name && touched.name && (
                      <div className="mt-1 flex items-center gap-1 text-destructive text-sm">
                        <AlertCircle size={14} />
                        <span>{errors.name}</span>
                      </div>
                    )}
                  </div>

                  {/* Environment Variables */}
                  <div>
                    <div className="flex justify-between items-center mb-2">
                      <label className="block text-sm font-medium text-foreground/80">
                        Environment Variables
                      </label>
                      <button
                        type="button"
                        onClick={() => setEnv([...env, { key: "", value: "" }])}
                        className="text-sm text-primary hover:text-primary/80 flex items-center gap-1
                                 px-2 py-1 rounded-md hover:bg-primary/10 transition-colors"
                        disabled={isSubmitting}
                      >
                        <Plus size={14} />
                        Add Variable
                      </button>
                    </div>
                    <div className="space-y-2 max-h-64 overflow-y-auto pr-1">
                      {env.map((item, index) => (
                        <div
                          key={index}
                          className="flex gap-2 items-center p-2 rounded-lg bg-muted/30 
                                   group hover:bg-muted/50 transition-colors"
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
                            className="flex-1 min-w-0 px-3 py-1.5 bg-card/50 rounded-md border border-input
                                     focus:border-primary outline-none transition-colors"
                            disabled={isSubmitting}
                          />
                          <input
                            placeholder="Value"
                            value={item.value}
                            onChange={(e) => {
                              const newEnv = [...env];
                              newEnv[index].value = e.target.value;
                              setEnv(newEnv);
                            }}
                            className="flex-1 min-w-0 px-3 py-1.5 bg-card/50 rounded-md border border-input
                                     focus:border-primary outline-none transition-colors"
                            disabled={isSubmitting}
                          />
                          <button
                            type="button"
                            onClick={() => removeEnvVar(index)}
                            className="p-1.5 text-muted-foreground hover:text-destructive rounded-md flex-shrink-0
                                     opacity-0 group-hover:opacity-100 transition-all hover:bg-muted/50"
                            disabled={isSubmitting}
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
                      <label className="block text-sm font-medium text-foreground/80">
                        Arguments
                      </label>
                      <button
                        type="button"
                        onClick={() => setArguments([...args, ""])}
                        className="text-sm text-primary hover:text-primary/80 flex items-center gap-1
                                 px-2 py-1 rounded-md hover:bg-primary/10 transition-colors"
                        disabled={isSubmitting}
                      >
                        <Plus size={14} />
                        Add Argument
                      </button>
                    </div>
                    <div className="space-y-2 max-h-48 overflow-y-auto pr-1">
                      {args.map((arg, index) => (
                        <div
                          key={index}
                          className="flex items-center gap-2 p-2 rounded-lg bg-muted/30
                                   group hover:bg-muted/50 transition-colors"
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
                            className="flex-1 min-w-0 px-3 py-1.5 bg-card/50 rounded-md border border-input
                                     focus:border-primary outline-none transition-colors"
                            placeholder="Enter argument"
                            disabled={isSubmitting}
                          />
                          <button
                            type="button"
                            onClick={() => removeArg(index)}
                            className="p-1.5 text-muted-foreground hover:text-destructive rounded-md flex-shrink-0
                                     opacity-0 group-hover:opacity-100 transition-all hover:bg-muted/50"
                            disabled={isSubmitting}
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
              <div className="p-6">
                <div className="flex justify-end items-center gap-3">
                  <button
                    type="button"
                    onClick={onClose}
                    className="px-4 py-2 text-sm bg-muted/50 rounded-lg hover:bg-muted 
                             transition-colors disabled:opacity-50"
                    disabled={isSubmitting}
                  >
                    Cancel
                  </button>
                  <button
                    type="submit"
                    disabled={isSubmitting}
                    className="px-4 py-2 text-sm bg-primary text-primary-foreground rounded-lg hover:bg-primary/90 
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
                        <span>Create Worker</span>
                      </>
                    )}
                  </button>
                </div>
              </div>
            </Form>
          )}
        </Formik>
      </div>
    </div>
  );
};