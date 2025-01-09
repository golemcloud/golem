import {
  AlertCircle,
  Cloud,
  FileIcon,
  Loader2,
  Plus,
  Server,
  Upload,
  X,
} from "lucide-react";
import { useCreateComponent, useUpdateComponent } from "../../api/components";
import { useEffect, useRef, useState } from "react";

import { Component } from "../../types/api";
import toast from "react-hot-toast";

type ComponentType = "Durable" | "Ephemeral";

interface ComponentModalProps {
  isOpen: boolean;
  onClose: () => void;
  existingComponent?: Component;
}

export interface InputProps
  extends React.InputHTMLAttributes<HTMLInputElement> {
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

type DropzoneProps = {
  onFileDrop: (e: React.DragEvent) => void;
  onFileSelect: (e: React.ChangeEvent<HTMLInputElement>) => void;
  inputRef: React.RefObject<HTMLInputElement>;
  file: File | null | File[];
  onRemove: (index?: number) => void;
  isSubmitting: boolean;
  accept?: string;
  multiple?: boolean;
  dragActive: boolean;
  setDragActive: React.Dispatch<React.SetStateAction<boolean>>;
  placeholder: string;
};

const FileDropzone = ({
  onFileDrop,
  onFileSelect,
  inputRef,
  file,
  onRemove,
  isSubmitting,
  accept = "*",
  multiple = false,
  dragActive,
  setDragActive,
  placeholder,
}: DropzoneProps) => (
  <div
    onClick={() => !isSubmitting && inputRef.current?.click()}
    onDragOver={(e) => {
      e.preventDefault();
      if (!isSubmitting) {
        setDragActive(true);
      }
    }}
    onDragLeave={() => setDragActive(false)}
    onDrop={onFileDrop}
    className={`border-2 border-dashed rounded-lg p-6 text-center transition-all duration-200
            ${isSubmitting ? "cursor-not-allowed opacity-60" : "cursor-pointer hover:border-blue-400/50"} 
            ${dragActive ? "border-blue-500 bg-primary/10" : "border-gray-600"}`}
  >
    {file || (multiple && file?.length > 0) ? (
      <div className="space-y-2">
        {multiple ? (
          file.map((f: File, index: number) => (
            <div
              key={index}
              className="flex items-center justify-between bg-card/50 rounded-lg px-4 py-2"
            >
              <div className="flex items-center gap-2">
                <FileIcon size={16} className="text-primary" />
                <span className="text-sm truncate">{f.name}</span>
              </div>
              {!isSubmitting && (
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    onRemove(index);
                  }}
                  className="p-1 text-muted-foreground hover:text-red-400 rounded-md
                                             hover:bg-gray-600/50 transition-colors"
                >
                  <X size={14} />
                </button>
              )}
            </div>
          ))
        ) : (
          <div className="flex items-center justify-between bg-card/50 rounded-lg px-4 py-2">
            <div className="flex items-center gap-2">
              <FileIcon size={16} className="text-primary" />
              <span className="text-sm">{file.name}</span>
            </div>
            {!isSubmitting && (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  onRemove();
                }}
                className="p-1 text-muted-foreground hover:text-red-400 rounded-md
                                         hover:bg-gray-600/50 transition-colors"
              >
                <X size={14} />
              </button>
            )}
          </div>
        )}
      </div>
    ) : (
      <div className="space-y-3">
        <Upload className="h-8 w-8 mx-auto text-muted-foreground" />
        <div>
          <p className="text-sm text-gray-300">{placeholder}</p>
          <p className="text-xs text-muted-foreground mt-1">
            or click to browse
          </p>
        </div>
      </div>
    )}
    <input
      ref={inputRef}
      type="file"
      accept={accept}
      multiple={multiple}
      onChange={onFileSelect}
      className="hidden"
      disabled={isSubmitting}
    />
  </div>
);

const CreateComponentModal = ({
  isOpen,
  onClose,
  existingComponent,
}: ComponentModalProps) => {
  const isUpdateMode = !!existingComponent;
  const [dragActive, setDragActive] = useState(false);
  const [mainFile, setMainFile] = useState<File | null>(null);
  const [additionalFiles, setAdditionalFiles] = useState<File[]>([]);
  const [name, setName] = useState("");
  const [componentType, setComponentType] = useState<ComponentType>("Durable");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const mainInputRef = useRef<HTMLInputElement | null>(null);
  const additionalInputRef = useRef<HTMLInputElement | null>(null);

  const createComponent = useCreateComponent();
  const updateComponent = useUpdateComponent();

  useEffect(() => {
    if (existingComponent) {
      setName(existingComponent.componentName);
      setComponentType(existingComponent.componentType);
    }
  }, [existingComponent]);

  const handleMainFileDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragActive(false);
    const droppedFile = e.dataTransfer.files[0];
    if (droppedFile?.name.endsWith(".wasm")) {
      setMainFile(droppedFile);
    } else {
      toast.error("Please upload a .wasm file");
    }
  };

  const handleMainFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    const selectedFile = e.target.files?.[0] || null;
    if (selectedFile?.name.endsWith(".wasm")) {
      setMainFile(selectedFile);
    } else {
      toast.error("Please upload a .wasm file");
    }
  };

  const handleAdditionalFileSelect = (
    e: React.ChangeEvent<HTMLInputElement>,
  ) => {
    const newFiles = Array.from(e.target.files || []);
    setAdditionalFiles((prev) => [...prev, ...newFiles]);
  };

  const removeAdditionalFile = (index: number) => {
    setAdditionalFiles((prev) => prev.filter((_, i) => i !== index));
  };

  const handleSubmit = async () => {
    if (!name || (!mainFile && !isUpdateMode)) return;

    setIsSubmitting(true);
    const formData = new FormData();
    formData.append("name", name);
    formData.append("componentType", componentType);

    if (mainFile) {
      formData.append("component", mainFile);
    }

    additionalFiles.forEach((file) => {
      formData.append("files", file);
    });

    try {
      if (isUpdateMode && existingComponent) {
        await updateComponent.mutateAsync({
          componentId: existingComponent.versionedComponentId.componentId,
          formData,
        });
        toast.success("Component updated successfully");
      } else {
        await createComponent.mutateAsync(formData);
        toast.success("Component created successfully");
      }

      // Reset form
      setMainFile(null);
      setAdditionalFiles([]);
      setName("");
      setComponentType("Durable");
      setIsSubmitting(false);
      onClose();
    } catch (error) {
      setIsSubmitting(false);
      console.error(error);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
      <div className="bg-card rounded-xl p-6 max-w-md w-full shadow-xl">
        <div className="flex justify-between items-start mb-6">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-md bg-primary/10 text-primary">
              <Plus size={20} />
            </div>
            <div>
              <h2 className="text-xl font-semibold">
                {isUpdateMode ? "Update Component" : "Create New Component"}
              </h2>
              <p className="text-sm text-muted-foreground mt-1">
                Configure your component settings
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

        <div className="space-y-6">
          <Input
            label="Component Name"
            value={name}
            onChange={(e: React.ChangeEvent<HTMLInputElement>) =>
              setName(e.target.value)
            }
            placeholder="Enter component name"
            disabled={isSubmitting || isUpdateMode}
          />

          <div>
            <label className="block text-sm font-medium mb-1.5 text-gray-300">
              Component Type
            </label>
            <div className="grid grid-cols-2 gap-4">
              {[
                { value: "Durable", label: "Durable", icon: Server },
                { value: "Ephemeral", label: "Ephemeral", icon: Cloud },
              ].map((option) => (
                <button
                  key={option.value}
                  onClick={() =>
                    setComponentType(option.value as ComponentType)
                  }
                  className={`flex items-center gap-3 p-4 rounded-lg border-2 transition-all
                                             ${
                                               componentType === option.value
                                                 ? "border-blue-500 bg-primary/10"
                                                 : "border-gray-600 hover:border-gray-500"
                                             }`}
                  disabled={isSubmitting}
                >
                  <option.icon
                    className={
                      componentType === option.value
                        ? "text-primary"
                        : "text-muted-foreground"
                    }
                    size={20}
                  />
                  <span>{option.label}</span>
                </button>
              ))}
            </div>
          </div>

          <div className="space-y-4">
            <div>
              <label className="block text-sm font-medium mb-1.5 text-gray-300">
                WASM File
              </label>
              <FileDropzone
                onFileDrop={handleMainFileDrop}
                onFileSelect={handleMainFileSelect}
                inputRef={mainInputRef}
                file={mainFile}
                onRemove={() => {
                  setMainFile(null);
                  if (mainInputRef.current) {
                    mainInputRef.current.value = "";
                  }
                }}
                isSubmitting={isSubmitting}
                accept=".wasm"
                dragActive={dragActive}
                setDragActive={setDragActive}
                placeholder="Drag and drop your WASM file here"
              />
            </div>

            <div className="hidden">
              <label className="block text-sm font-medium mb-1.5 text-gray-300">
                Additional Files
              </label>
              <FileDropzone
                onFileSelect={handleAdditionalFileSelect}
                inputRef={additionalInputRef}
                file={additionalFiles}
                onRemove={removeAdditionalFile}
                isSubmitting={isSubmitting}
                multiple={true}
                dragActive={dragActive}
                setDragActive={setDragActive}
                placeholder="Add additional files"
              />
            </div>
          </div>

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
              disabled={!name || (!mainFile && !isUpdateMode) || isSubmitting}
              className="px-4 py-2 text-sm bg-primary rounded-lg hover:bg-blue-600 
                                     disabled:opacity-50 transition-colors flex items-center gap-2"
            >
              {isSubmitting ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  <span>{isUpdateMode ? "Updating..." : "Creating..."}</span>
                </>
              ) : (
                <>
                  <Plus size={16} />
                  <span>
                    {isUpdateMode ? "Update Component" : "Create Component"}
                  </span>
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

export default CreateComponentModal;
