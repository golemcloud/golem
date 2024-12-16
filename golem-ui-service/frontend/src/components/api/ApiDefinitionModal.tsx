import { useCreateApiDefinition, useUpdateApiDefinition } from '../../api/api-definitions';

import { X } from 'lucide-react';
import toast from 'react-hot-toast';
import { useState } from 'react';

interface ApiDefinitionModalProps {
    isOpen: boolean;
    onClose: () => void;
    onApiDefinitionCreated: (apiDefinitionId: string) => void;
}

export const ApiDefinitionModal = ({ isOpen, onClose, onApiDefinitionCreated }: ApiDefinitionModalProps) => {
    const [name, setName] = useState('');
    const [version, setVersion] = useState('');
    const [isSubmitting, setIsSubmitting] = useState(false);

    const createDefinition = useCreateApiDefinition();

    const handleSubmit = async () => {
        if (!name || !version) return;

        setIsSubmitting(true);
        const apiDefinition = {
            id: name,
            version,
            draft: true,
            routes: []
        };

        try {
            const createdDefinition = await createDefinition.mutateAsync(apiDefinition);
            toast.success('API definition created successfully');
            resetForm();
            onApiDefinitionCreated(createdDefinition.id);
            onClose();
        } catch (error) {
            toast.error('Failed to create API definition');
        } finally {
            setIsSubmitting(false);
        }
    };

    const resetForm = () => {
        setName('');
        setVersion('');
    };

    if (!isOpen) return null;

    return (
        <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4 z-50">
            <div className="bg-gray-800 rounded-lg p-6 max-w-md w-full">
                <div className="flex justify-between items-start mb-4">
                    <h2 className="text-xl font-semibold">Create API Definition</h2>
                    <button onClick={onClose} className="text-gray-400 hover:text-gray-300">
                        <X size={20} />
                    </button>
                </div>

                <div className="space-y-4">
                    <div>
                        <label className="block text-sm font-medium mb-1">Name</label>
                        <input
                            type="text"
                            value={name}
                            onChange={(e) => setName(e.target.value)}
                            className="w-full px-3 py-2 bg-gray-700 rounded-md"
                            disabled={isSubmitting}
                        />
                    </div>
                    <div>  
                        <label className="block text-sm font-medium mb-1">Version</label>
                        <input
                            type="text"
                            value={version}
                            onChange={(e) => setVersion(e.target.value)}
                            className="w-full px-3 py-2 bg-gray-700 rounded-md"
                            disabled={isSubmitting}
                        />
                    </div>

                    <div className="flex justify-end mt-6">
                        <button
                            onClick={onClose}
                            className="px-4 py-2 text-sm bg-gray-700 rounded-md hover:bg-gray-600 mr-2"
                            disabled={isSubmitting}
                        >
                            Cancel
                        </button>
                        <button
                            onClick={handleSubmit}
                            disabled={!name || !version || isSubmitting}
                            className="px-4 py-2 text-sm bg-blue-500 rounded-md hover:bg-blue-600 disabled:opacity-50"
                        >
                            {isSubmitting ? 'Creating...' : 'Create Definition'}
                        </button>
                    </div>
                </div>
            </div>
        </div>
    );
};