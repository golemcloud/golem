import { useCreateWorker } from "../../api/workers";
import { useState } from "react";

export const CreateWorkerModal = ({
    isOpen,
    onClose,
    componentId
}: {
    isOpen: boolean;
    onClose: () => void;
    componentId: string;
}) => {
    const [name, setName] = useState('');
    const [env, setEnv] = useState<{ key: string; value: string }[]>([{ key: '', value: '' }]);
    const [args, setArguments] = useState<string[]>([]);
    const createWorker = useCreateWorker(componentId);

    const handleSubmit = () => {
        const envRecord = env.reduce((acc, { key, value }) => {
            if (key) acc[key] = value;
            return acc;
        }, {} as Record<string, string>);

        createWorker.mutate({
            name: name.replace(/ /g, "-"),
            env: envRecord,
            args
        }, {
            onSuccess: onClose
        });
    };

    return isOpen ? (
        <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4">
            <div className="bg-gray-800 rounded-lg p-6 max-w-md w-full">
                <h2 className="text-xl font-semibold mb-4">Create New Worker</h2>

                <div className="space-y-4">
                    <div>
                        <label className="block text-sm font-medium mb-1">Worker Name</label>
                        <input
                            type="text"
                            value={name}
                            onChange={(e) => setName(e.target.value)}
                            className="w-full px-3 py-2 bg-gray-700 rounded-md"
                            placeholder="Enter worker name"
                        />
                    </div>

                    <div>
                        <label className="block text-sm font-medium mb-2">Environment Variables</label>
                        {env.map((item, index) => (
                            <div key={index} className="flex gap-2 mb-2">
                                <input
                                    placeholder="Key"
                                    value={item.key}
                                    onChange={(e) => {
                                        const newEnv = [...env];
                                        newEnv[index].key = e.target.value;
                                        setEnv(newEnv);
                                    }}
                                    className="flex-1 px-3 py-2 bg-gray-700 rounded-md"
                                />
                                <input
                                    placeholder="Value"
                                    value={item.value}
                                    onChange={(e) => {
                                        const newEnv = [...env];
                                        newEnv[index].value = e.target.value;
                                        setEnv(newEnv);
                                    }}
                                    className="flex-1 px-3 py-2 bg-gray-700 rounded-md"
                                />
                            </div>
                        ))}
                        <button
                            onClick={() => setEnv([...env, { key: '', value: '' }])}
                            className="text-sm text-blue-400 hover:text-blue-300"
                        >
                            + Add Environment Variable
                        </button>

                    </div>

                    {/* Arguments */}
                    <div>
                        <label className="block text-sm font-medium mb-2">Arguments</label>
                        {args.map((arg, index) => (
                            <input
                                key={index}
                                value={arg}
                                onChange={(e) => {
                                    const newArgs = [...args];
                                    newArgs[index] = e.target.value;
                                    setArguments(newArgs);
                                }}
                                className="w-full px-3 py-2 bg-gray-700 rounded-md"
                                placeholder="Enter argument"
                            />
                        ))}
                        <button
                            onClick={() => setArguments([...args, ''])}
                            className="text-sm text-blue-400 hover:text-blue-300"
                        >
                            + Add Argument
                        </button>
                    </div>

                    <div className="flex justify-end space-x-3 mt-6">
                        <button
                            onClick={onClose}
                            className="px-4 py-2 text-sm bg-gray-700 rounded-md hover:bg-gray-600"
                        >
                            Cancel
                        </button>
                        <button
                            onClick={handleSubmit}
                            disabled={!name}
                            className="px-4 py-2 text-sm bg-blue-500 rounded-md hover:bg-blue-600 disabled:opacity-50"
                        >
                            Create Worker
                        </button>
                    </div>
                </div>
            </div>
        </div>
    ) : null;
};