import {
    Box,
    ChevronDown,
    Code2,
    Globe,
    Route,
    Webhook,
    X,
} from "lucide-react";
import { useEffect, useState } from "react";

import toast from "react-hot-toast";
import { useComponents } from "../../api/components";

const HTTP_METHODS = [
    { value: "Get", color: "text-green-500 bg-green-500/10" },
    { value: "Post", color: "text-blue-500 bg-blue-500/10" },
    { value: "Put", color: "text-yellow-500 bg-yellow-500/10" },
    { value: "Delete", color: "text-red-500 bg-red-500/10" },
    { value: "Patch", color: "text-purple-500 bg-purple-500/10" },
    { value: "Head", color: "text-gray-500 bg-gray-500/10" },
    { value: "Options", color: "text-gray-500 bg-gray-500/10" },
];

interface RouteModalProps {
    isOpen: boolean;
    onClose: () => void;
    onSave: (route: any) => void;
    existingRoute?: any;
}

export const RouteModal = ({
    isOpen,
    onClose,
    onSave,
    existingRoute,
}: RouteModalProps) => {
    const [method, setMethod] = useState("Get");
    const [path, setPath] = useState("");
    const [selectedComponent, setSelectedComponent] = useState<any>(null);
    const [selectedVersion, setSelectedVersion] = useState<number>(0);
    const [workerName, setWorkerName] = useState("");
    const [response, setResponse] = useState("");
    const [showMethodDropdown, setShowMethodDropdown] = useState(false);

    const { data: components } = useComponents();

    useEffect(() => {
        if (existingRoute) {
            setMethod(existingRoute.method);
            setPath(existingRoute.path);
            setWorkerName(existingRoute.binding.workerName);
            setResponse(existingRoute.binding.response);
            setSelectedComponent(
                components?.find(
                    (c) =>
                        c.versionedComponentId.componentId ===
                        existingRoute.binding.componentId.componentId &&
                        c.versionedComponentId.version ===
                        existingRoute.binding.componentId.version
                )
            );
            setSelectedVersion(existingRoute.binding.componentId.version);
        }
    }, [existingRoute]);

    const handleSave = () => {
        if (!path || !selectedComponent || !workerName) {
            toast.error("Please fill in all required fields");
            return;
        }

        const route = {
            method,
            path,
            binding: {
                componentId: {
                    componentId: selectedComponent.versionedComponentId.componentId,
                    version: selectedVersion,
                },
                workerName,
                response,
                bindingType: "default",
            },
        };

        onSave(route);
        onClose();
    };

    const getMethodColor = (methodValue: string) => {
        return (
            HTTP_METHODS.find((m) => m.value === methodValue)?.color ||
            "text-gray-500 bg-gray-500/10"
        );
    };

    if (!isOpen) return null;

    return (
        <div className='fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4 z-50'>
            <div className='bg-gray-800 rounded-lg p-6 max-w-xl w-full'>
                <div className='flex justify-between items-start mb-6'>
                    <h2 className='text-xl font-semibold flex items-center gap-2'>
                        <Route className='h-5 w-5 text-blue-400' />
                        {existingRoute ? "Edit Route" : "Add New Route"}
                    </h2>
                    <button
                        onClick={onClose}
                        className='text-gray-400 hover:text-gray-300'>
                        <X size={20} />
                    </button>
                </div>

                <div className='space-y-6'>
                    {/* Method & Path */}
                    <div className='flex gap-4'>
                        <div className='relative'>
                            <label className='block text-sm font-medium mb-1'>Method</label>
                            <button
                                onClick={() => setShowMethodDropdown(!showMethodDropdown)}
                                className={`flex items-center gap-2 px-3 py-2 rounded-md ${getMethodColor(
                                    method
                                )} w-32 justify-between`}>
                                {method}
                                <ChevronDown size={16} />
                            </button>

                            {showMethodDropdown && (
                                <div className='absolute top-full mt-1 w-full bg-gray-700 rounded-md shadow-lg py-1 z-10'>
                                    {HTTP_METHODS.map(({ value, color }) => (
                                        <button
                                            key={value}
                                            onClick={() => {
                                                setMethod(value);
                                                setShowMethodDropdown(false);
                                            }}
                                            className={`w-full text-left px-3 py-2 hover:bg-gray-600 ${color}`}>
                                            {value}
                                        </button>
                                    ))}
                                </div>
                            )}
                        </div>

                        <div className='flex-1'>
                            <label className='block text-sm font-medium mb-1'>Path</label>
                            <div className='relative'>
                                <Globe className='absolute left-3 top-2.5 h-4 w-4 text-gray-400' />
                                <input
                                    type='text'
                                    value={path}
                                    onChange={(e) => setPath(e.target.value)}
                                    className='w-full pl-10 pr-3 py-2 bg-gray-700 rounded-md'
                                    placeholder='/api/v1/resource'
                                />
                            </div>
                        </div>
                    </div>

                    {/* Component Selection */}
                    <div>
                        <label className='block text-sm font-medium mb-1'>Component</label>
                        <div className='grid grid-cols-2 gap-4'>
                            <select
                                value={
                                    selectedComponent?.versionedComponentId.componentId +
                                    ":" +
                                    selectedComponent?.versionedComponentId.version || ""
                                }
                                onChange={(e) => {
                                    let cId = e.target.value.split(":")[0];
                                    let version = Number(e.target.value.split(":")[1]);
                                    const component = components?.find(
                                        (c) =>
                                            c.versionedComponentId.componentId == cId &&
                                            c.versionedComponentId.version == version
                                    );
                                    setSelectedComponent(component);
                                    setSelectedVersion(
                                        component?.versionedComponentId.version || 0
                                    );
                                }}
                                className='bg-gray-700 rounded-md px-3 py-2'>
                                <option value=''>Select Component</option>
                                {components?.map((component) => (
                                    <option
                                        key={
                                            component.versionedComponentId.componentId +
                                            component.versionedComponentId.version
                                        }
                                        value={
                                            component.versionedComponentId.componentId +
                                            ":" +
                                            component.versionedComponentId.version
                                        }>
                                        {component.componentName}
                                    </option>
                                ))}
                            </select>

                            <select
                                value={selectedVersion}
                                onChange={(e) => setSelectedVersion(Number(e.target.value))}
                                className='bg-gray-700 rounded-md px-3 py-2'
                                disabled={!selectedComponent}
                                key={
                                    selectedComponent?.versionedComponentId?.componentId +
                                    selectedComponent?.versionedComponentId?.version
                                }>
                                <option value={selectedComponent?.versionedComponentId.version}>
                                    Version {selectedComponent?.versionedComponentId.version}
                                </option>
                            </select>
                        </div>
                    </div>

                    {/* Worker Name */}
                    <div>
                        <label className='block text-sm font-medium mb-1'>
                            Worker Name
                        </label>
                        <div className='relative'>
                            <Box className='absolute left-3 top-2.5 h-4 w-4 text-gray-400' />
                            <input
                                type='text'
                                value={workerName}
                                onChange={(e) => setWorkerName(e.target.value)}
                                className='w-full pl-10 pr-3 py-2 bg-gray-700 rounded-md'
                                placeholder='worker-name'
                            />
                        </div>
                    </div>

                    {/* Response */}
                    <div>
                        <label className='block text-sm font-medium mb-1'>Response</label>
                        <div className='relative'>
                            <Code2 className='absolute left-3 top-2.5 h-4 w-4 text-gray-400' />
                            <input
                                type='text'
                                value={response}
                                onChange={(e) => setResponse(e.target.value)}
                                className='w-full pl-10 pr-3 py-2 bg-gray-700 rounded-md'
                                placeholder='Response type (optional)'
                            />
                        </div>
                    </div>

                    <div className='flex justify-end space-x-3 mt-6'>
                        <button
                            onClick={onClose}
                            className='px-4 py-2 text-sm bg-gray-700 rounded-md hover:bg-gray-600'>
                            Cancel
                        </button>
                        <button
                            onClick={handleSave}
                            className='px-4 py-2 text-sm bg-blue-500 rounded-md hover:bg-blue-600 flex items-center gap-2'>
                            <Webhook size={16} />
                            Save Route
                        </button>
                    </div>
                </div>
            </div>
        </div>
    );
};

export default RouteModal;
