import { Folder, Upload, X } from 'lucide-react';
import { useCreateComponent, useUpdateComponent } from '../../api/components';
import { useEffect, useRef, useState } from 'react';

import { Component } from '../../types/api';
import toast from 'react-hot-toast';

type ComponentType = 'Durable' | 'Ephemeral';

interface ComponentModalProps {
    isOpen: boolean;
    onClose: () => void;
    existingComponent?: Component; // Pass this for update mode
}

const CreateComponentModal = ({ isOpen, onClose, existingComponent }: ComponentModalProps) => {
    const isUpdateMode = !!existingComponent;
    const [dragActive, setDragActive] = useState(false);
    const [mainFile, setMainFile] = useState<File | null>(null);
    const [additionalFiles, setAdditionalFiles] = useState<File[]>([]);
    const [name, setName] = useState('');
    const [componentType, setComponentType] = useState<ComponentType>('Durable');
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
        if (droppedFile?.name.endsWith('.wasm')) {
            setMainFile(droppedFile);
        } else {
            toast.error('Please upload a .wasm file');
        }
    };

    const handleMainFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
        const selectedFile = e.target.files?.[0] || null;
        if (selectedFile?.name.endsWith('.wasm')) {
            setMainFile(selectedFile);
        } else {
            toast.error('Please upload a .wasm file');
        }
    };

    const handleAdditionalFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
        const newFiles = Array.from(e.target.files || []);
        setAdditionalFiles(prev => [...prev, ...newFiles]);
    };

    const removeAdditionalFile = (index: number) => {
        setAdditionalFiles(prev => prev.filter((_, i) => i !== index));
    };

    const handleSubmit = async () => {
        if (!name || (!mainFile && !isUpdateMode)) return;

        setIsSubmitting(true);
        const formData = new FormData();
        formData.append('name', name);
        formData.append('componentType', componentType);

        if (mainFile) {
            formData.append('component', mainFile);
        }

        // Append additional files
        additionalFiles.forEach(file => {
            formData.append('files', file);
        });

        try {
            if (isUpdateMode && existingComponent) {
                await updateComponent.mutateAsync({
                    componentId: existingComponent.versionedComponentId.componentId,
                    formData
                });
                toast.success('Component updated successfully');
            } else {
                await createComponent.mutateAsync(formData);
                toast.success('Component created successfully');
            }

            // Reset form
            setMainFile(null);
            setAdditionalFiles([]);
            setName('');
            setComponentType('Durable');
            setIsSubmitting(false);
            onClose();
        } catch (error) {
            toast.error(`Failed to ${isUpdateMode ? 'update' : 'create'} component`);
            setIsSubmitting(false);
            console.error(`Failed to ${isUpdateMode ? 'update' : 'create'} component:`, error);
        }
    };

    return isOpen ? (
        <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4 z-50">
            <div className="bg-gray-800 rounded-lg p-6 max-w-md w-full">
                <h2 className="text-xl font-semibold mb-4">Create New Component</h2>

                <div className="space-y-4">
                    <div>
                        <label className="block text-sm font-medium mb-1">Component Name</label>
                        <input
                            type="text"
                            value={name}
                            onChange={(e) => setName(e.target.value)}
                            className="w-full px-3 py-2 bg-gray-700 rounded-md focus:ring-2 focus:ring-blue-500"
                            placeholder="Enter component name"
                            disabled={isSubmitting || isUpdateMode}
                        />
                    </div>

                    <div>
                        <label className="block text-sm font-medium mb-1">Component Type</label>
                        <select
                            value={componentType}
                            onChange={(e) => setComponentType(e.target.value as ComponentType)}
                            className="w-full px-3 py-2 bg-gray-700 rounded-md focus:ring-2 focus:ring-blue-500"
                            disabled={isSubmitting}
                        >
                            <option value="Durable">Durable</option>
                            <option value="Ephemeral">Ephemeral</option>
                        </select>
                    </div>

                    <div>
                        <label className="block text-sm font-medium mb-1">WASM File</label>
                        <div
                            onClick={() => !isSubmitting && mainInputRef.current?.click()}
                            onDragOver={(e) => {
                                e.preventDefault();
                                !isSubmitting && setDragActive(true);
                            }}
                            onDragLeave={() => setDragActive(false)}
                            onDrop={handleMainFileDrop}
                            className={`border-2 border-dashed rounded-lg p-8 text-center 
                ${isSubmitting ? 'cursor-not-allowed opacity-60' : 'cursor-pointer'} 
                ${dragActive ? 'border-blue-500 bg-blue-500 bg-opacity-10' : 'border-gray-600'}`}
                        >
                            {mainFile ? (
                                <div className="flex items-center justify-center space-x-2">
                                    <Folder className="h-5 w-5" />
                                    <span>{mainFile.name}</span>
                                    {!isSubmitting && (
                                        <button
                                            onClick={(e) => {
                                                e.stopPropagation();
                                                setMainFile(null);
                                                if (mainInputRef.current) {
                                                    mainInputRef.current.value = '';
                                                }
                                            }}
                                            className="ml-2 text-red-400 hover:text-red-300"
                                        >
                                            <X size={16} />
                                        </button>
                                    )}
                                </div>
                            ) : (
                                <div className="space-y-2">
                                    <Upload className="h-8 w-8 mx-auto text-gray-400" />
                                    <div>
                                        <p className="text-sm">Drag and drop your WASM file here</p>
                                        <p className="text-xs text-gray-400">or click to browse</p>
                                    </div>
                                </div>
                            )}
                            <input
                                ref={mainInputRef}
                                type="file"
                                accept=".wasm"
                                onChange={handleMainFileSelect}
                                className="hidden"
                                disabled={isSubmitting}
                            />
                        </div>
                    </div>

                    <div>
                        <label className="block text-sm font-medium mb-1">Additional Files</label>
                        <div className="space-y-2">
                            {additionalFiles.map((file, index) => (
                                <div
                                    key={index}
                                    className="flex items-center justify-between bg-gray-700 rounded-md px-3 py-2"
                                >
                                    <span className="text-sm truncate">{file.name}</span>
                                    {!isSubmitting && (
                                        <button
                                            onClick={() => removeAdditionalFile(index)}
                                            className="text-red-400 hover:text-red-300"
                                        >
                                            <X size={16} />
                                        </button>
                                    )}
                                </div>
                            ))}
                            <button
                                onClick={() => !isSubmitting && additionalInputRef.current?.click()}
                                className="w-full px-3 py-2 text-sm border border-dashed border-gray-600 rounded-md hover:border-gray-500 disabled:opacity-50"
                                disabled={isSubmitting}
                            >
                                Add Files
                            </button>
                            <input
                                ref={additionalInputRef}
                                type="file"
                                multiple
                                onChange={handleAdditionalFileSelect}
                                className="hidden"
                                disabled={isSubmitting}
                            />
                        </div>
                    </div>

                    <div className="flex justify-end space-x-3 mt-6">
                        <button
                            onClick={onClose}
                            className="px-4 py-2 text-sm bg-gray-700 rounded-md hover:bg-gray-600 disabled:opacity-50"
                            disabled={isSubmitting}
                        >
                            Cancel
                        </button>
                        <button
                            onClick={handleSubmit}
                            disabled={!name || !mainFile || isSubmitting}
                            className="px-4 py-2 text-sm bg-blue-500 rounded-md hover:bg-blue-600 disabled:opacity-50 flex items-center gap-2"
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
                                    Creating...
                                </>
                            ) : (
                                isUpdateMode ? 'Update Component' : 'Create Component'
                            )}
                        </button>
                    </div>
                </div>
            </div>
        </div>
    ) : null;
};

export default CreateComponentModal;