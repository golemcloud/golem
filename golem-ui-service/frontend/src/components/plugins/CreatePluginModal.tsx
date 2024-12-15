import { AlertCircle, ArrowRight, Code, Download, Globe, Loader2, Plus, Server, Settings, Upload, X } from 'lucide-react';

import toast from 'react-hot-toast';
import { useComponents } from '../../api/components';
import { useCreatePlugin } from '../../api/plugins';
import { useState } from 'react';

type PluginType = 'OplogProcessor' | 'ComponentTransformer';

interface CreatePluginModalProps {
    isOpen: boolean;
    onClose: () => void;
}

const Input = ({ label, error, ...props }: any) => (
    <div>
        <label className="block text-sm font-medium mb-1.5 text-gray-300">{label}</label>
        <input
            {...props}
            className="w-full px-4 py-2.5 bg-gray-700/50 rounded-lg border border-gray-600 focus:border-blue-500 
                     focus:ring-1 focus:ring-blue-500 outline-none transition duration-200
                     disabled:opacity-50 disabled:cursor-not-allowed"
        />
        {error && (
            <div className="mt-1 flex items-center gap-1 text-red-400 text-sm">
                <AlertCircle size={14} />
                <span>{error}</span>
            </div>
        )}
    </div>
);

export const CreatePluginModal = ({ isOpen, onClose }: CreatePluginModalProps) => {
    const [name, setName] = useState('');
    const [version, setVersion] = useState('');
    const [description, setDescription] = useState('');
    const [homepage, setHomepage] = useState('');
    const [type, setType] = useState<PluginType>('ComponentTransformer');
    const [isSubmitting, setIsSubmitting] = useState(false);

    // OplogProcessor fields
    const [selectedComponentId, setSelectedComponentId] = useState('');
    const [selectedVersion, setSelectedVersion] = useState<number>(0);

    // ComponentTransformer fields
    const [jsonSchema, setJsonSchema] = useState('');
    const [validateUrl, setValidateUrl] = useState('');
    const [transformUrl, setTransformUrl] = useState('');

    const { data: components } = useComponents();
    const createPlugin = useCreatePlugin();

    const handleSubmit = async () => {
        setIsSubmitting(true);

        const pluginData = {
            name,
            version,
            description,
            specs: type === 'OplogProcessor'
                ? {
                    type: 'OplogProcessor',
                    componentId: selectedComponentId,
                    componentVersion: selectedVersion,
                }
                : {
                    type: 'ComponentTransformer',
                    jsonSchema,
                    validateUrl,
                    transformUrl,
                },
            scope: {
                type: 'Global'
            },
            icon: [0],
            homepage
        };

        try {
            await createPlugin.mutateAsync(pluginData);
            toast.success('Plugin created successfully');
            resetForm();
            onClose();
        } catch (error) {
            toast.error('Failed to create plugin');
        } finally {
            setIsSubmitting(false);
        }
    };

    const resetForm = () => {
        setName('');
        setVersion('');
        setDescription('');
        setType('ComponentTransformer');
        setSelectedComponentId('');
        setSelectedVersion(0);
        setJsonSchema('');
        setValidateUrl('');
        setTransformUrl('');
    };

    if (!isOpen) return null;

    return (
        <div className="fixed inset-0 bg-black/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
            <div className="bg-gray-800 rounded-xl p-6 max-w-2xl w-full shadow-xl">
                <div className="flex justify-between items-start mb-6">
                    <div className="flex items-center gap-3">
                        <div className="p-2 rounded-md bg-blue-500/10 text-blue-400">
                            <Plus size={20} />
                        </div>
                        <div>
                            <h2 className="text-xl font-semibold">Create New Plugin</h2>
                            <p className="text-sm text-gray-400 mt-1">Configure your plugin settings</p>
                        </div>
                    </div>
                    <button
                        onClick={onClose}
                        className="text-gray-400 hover:text-gray-300 p-1 hover:bg-gray-700/50 rounded-md transition-colors"
                    >
                        <X size={20} />
                    </button>
                </div>

                <div className="space-y-6">
                    <div className="grid grid-cols-2 gap-4">
                        <Input
                            label="Plugin Name"
                            value={name}
                            onChange={(e: any) => setName(e.target.value)}
                            disabled={isSubmitting}
                            placeholder="Enter plugin name"
                        />
                        <Input
                            label="Version"
                            value={version}
                            onChange={(e: any) => setVersion(e.target.value)}
                            disabled={isSubmitting}
                            placeholder="e.g., 1.0.0"
                        />
                    </div>

                    <Input
                        label="Description"
                        value={description}
                        onChange={(e: any) => setDescription(e.target.value)}
                        disabled={isSubmitting}
                        placeholder="Brief description of your plugin"
                    />

                    <Input
                        label="Homepage"
                        value={homepage}
                        onChange={(e: any) => setHomepage(e.target.value)}
                        disabled={isSubmitting}
                        placeholder="https://"
                    />

                    <div>
                        <label className="block text-sm font-medium mb-1.5 text-gray-300">Plugin Type</label>
                        <div className="grid grid-cols-2 gap-4">
                            {[
                                { value: 'OplogProcessor', label: 'Oplog Processor', icon: Server },
                                { value: 'ComponentTransformer', label: 'Component Transformer', icon: Settings }
                            ].map(option => (
                                <button
                                    key={option.value}
                                    onClick={() => setType(option.value as PluginType)}
                                    className={`flex items-center gap-3 p-4 rounded-lg border-2 transition-all
                                             ${type === option.value 
                                                 ? 'border-blue-500 bg-blue-500/10' 
                                                 : 'border-gray-600 hover:border-gray-500'}`}
                                    disabled={isSubmitting}
                                >
                                    <option.icon className={type === option.value ? 'text-blue-400' : 'text-gray-400'} size={20} />
                                    <span>{option.label}</span>
                                </button>
                            ))}
                        </div>
                    </div>

                    {type === 'OplogProcessor' ? (
                        <div className="space-y-4 border-t border-gray-700 pt-4">
                            <div>
                                <label className="block text-sm font-medium mb-1.5 text-gray-300">Component</label>
                                <select
                                    value={selectedComponentId}
                                    onChange={(e) => setSelectedComponentId(e.target.value)}
                                    className="w-full px-4 py-2.5 bg-gray-700/50 rounded-lg border border-gray-600 
                                             focus:border-blue-500 outline-none"
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
                                    onChange={(e: any) => setSelectedVersion(Number(e.target.value))}
                                    disabled={isSubmitting}
                                    min="0"
                                />
                            )}
                        </div>
                    ) : (
                        <div className="space-y-4 border-t border-gray-700 pt-4">
                            <div>
                                <label className="block text-sm font-medium mb-1.5 text-gray-300">JSON Schema</label>
                                <textarea
                                    value={jsonSchema}
                                    onChange={(e) => setJsonSchema(e.target.value)}
                                    className="w-full px-4 py-2.5 bg-gray-700/50 rounded-lg border border-gray-600 
                                             focus:border-blue-500 outline-none font-mono text-sm h-32 resize-none"
                                    placeholder="{}"
                                    disabled={isSubmitting}
                                />
                            </div>
                            <Input
                                label="Validate URL"
                                type="url"
                                value={validateUrl}
                                onChange={(e: any) => setValidateUrl(e.target.value)}
                                disabled={isSubmitting}
                                placeholder="https://"
                            />
                            <Input
                                label="Transform URL"
                                type="url"
                                value={transformUrl}
                                onChange={(e: any) => setTransformUrl(e.target.value)}
                                disabled={isSubmitting}
                                placeholder="https://"
                            />
                        </div>
                    )}

                    <div className="flex justify-end items-center gap-3 pt-2">
                        <button
                            onClick={onClose}
                            className="px-4 py-2 text-sm bg-gray-700 rounded-lg hover:bg-gray-600 transition-colors
                                     disabled:opacity-50"
                            disabled={isSubmitting}
                        >
                            Cancel
                        </button>
                        <button
                            onClick={handleSubmit}
                            disabled={!name || !version || isSubmitting}
                            className="px-4 py-2 text-sm bg-blue-500 rounded-lg hover:bg-blue-600 disabled:opacity-50
                                     transition-colors flex items-center gap-2"
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