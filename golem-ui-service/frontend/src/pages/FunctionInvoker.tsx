import {
    AlertCircle,
    ArrowLeft,
    Code2,
    Loader2,
    Play,
    SquareFunction,
    Terminal,
} from "lucide-react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { useInvokeWorker, useWorker } from "../api/workers";

import { GolemError } from "../types/error";
import { apiClient } from "../lib/api-client";
import toast from "react-hot-toast";
import { useComponent } from "../api/components";
import { useMutation } from "@tanstack/react-query";
import { useState } from "react";

const TypeBadge = ({ type }: { type: string }) => (
    <span className='px-2 py-0.5 rounded-full text-xs bg-blue-500/10 text-blue-400 font-mono'>
        {type}
    </span>
);

interface TypeDefinition {
    type: string;
    fields?: Array<{
        name: string;
        typ: TypeDefinition;
    }>;
    cases?: Array<{
        name: string;
        typ: TypeDefinition;
    }>;
}

const RecursiveParameterInput = ({
    name,
    typeDef,
    value,
    onChange,
    path = "",
}: {
    name: string;
    typeDef: TypeDefinition;
    value: any;
    onChange: (path: string, value: any) => void;
    path?: string;
}) => {
    const currentPath = path ? `${path}.${name}` : name;

    const handleValueChange = (newValue: any) => {
        // Convert strings to appropriate types based on the type definition
        let processedValue = newValue;
        switch (typeDef.type.toLowerCase()) {
            case "f32":
            case "f64":
                processedValue = parseFloat(newValue) || 0;
                break;
            case "i32":
            case "i64":
            case "u32":
            case "u64":
                processedValue = parseInt(newValue) || 0;
                break;
            case "bool":
                processedValue = newValue === "true";
                break;
        }
        onChange(currentPath, processedValue);
    };

    const renderInput = () => {
        switch (typeDef.type) {
            case "Record":
                return (
                    <div className='space-y-4 bg-card/60 p-4 rounded-lg'>
                        {typeDef.fields?.map((field) => (
                            <div key={field.name}>
                                <RecursiveParameterInput
                                    name={field.name}
                                    typeDef={field.typ}
                                    value={value?.[field.name]}
                                    onChange={onChange}
                                    path={currentPath}
                                />
                            </div>
                        ))}
                    </div>
                );

            case "Variant":
                return (
                    <div className='space-y-4'>
                        <select
                            className='w-full bg-card/70 border rounded-lg p-2 text-sm'
                            value={value?.type || ""}
                            onChange={(e) => handleValueChange({ type: e.target.value })}>
                            <option value=''>Select variant...</option>
                            {typeDef.cases?.map((caseItem) => (
                                <option key={caseItem.name} value={caseItem.name}>
                                    {caseItem.name}
                                </option>
                            ))}
                        </select>
                        {value?.type &&
                            typeDef.cases?.find((c) => c.name === value.type)?.typ && (
                                <RecursiveParameterInput
                                    name='value'
                                    typeDef={
                                        typeDef.cases.find((c) => c.name === value.type)!.typ
                                    }
                                    value={value.value}
                                    onChange={(_, newValue) =>
                                        handleValueChange({ type: value.type, value: newValue })
                                    }
                                    path={currentPath}
                                />
                            )}
                    </div>
                );

            case "List":
            case "Array":
                return (
                    <div className='space-y-2'>
                        {Array.isArray(value) &&
                            value.map((item, index) => (
                                <div key={index} className='flex gap-2'>
                                    <RecursiveParameterInput
                                        name={index.toString()}
                                        typeDef={typeDef.typ}
                                        value={item}
                                        onChange={(_, newValue) => {
                                            const newArray = [...(value || [])];
                                            newArray[index] = newValue;
                                            handleValueChange(newArray);
                                        }}
                                        path={currentPath}
                                    />
                                    <button
                                        onClick={() => {
                                            const newArray = value.filter((_, i) => i !== index);
                                            handleValueChange(newArray);
                                        }}
                                        className='p-2 text-destructive hover:text-destructive/80 text-sm rounded-lg'>
                                        ✕
                                    </button>
                                </div>
                            ))}
                        <button
                            onClick={() => handleValueChange([...(value || []), null])}
                            className='text-primary hover:text-primary/80 text-sm'>
                            + Add Item
                        </button>
                    </div>
                );

            case "Bool":
                return (
                    <select
                        className='w-full bg-card/90 border border-foreground rounded-lg p-2 text-sm'
                        value={value?.toString()}
                        onChange={(e) => handleValueChange(e.target.value === "true")}>
                        <option value='true'>true</option>
                        <option value='false'>false</option>
                    </select>
                );

            default:
                return (
                    <input
                        type={
                            typeDef.type.toLowerCase().includes("32") ||
                                typeDef.type.toLowerCase().includes("64")
                                ? "number"
                                : "text"
                        }
                        className='w-full bg-card/80 border border-foreground rounded-lg p-2 font-mono text-sm'
                        placeholder={`Enter ${name}...`}
                        value={value || ""}
                        onChange={(e) => handleValueChange(e.target.value)}
                        step={typeDef.type.toLowerCase().startsWith("f") ? "0.01" : "1"}
                    />
                );
        }
    };

    return (
        <div className='space-y-2'>
            <label className='flex items-center gap-2 text-sm font-medium'>
                {name}
                <TypeBadge type={typeDef.type} />
            </label>
            {renderInput()}
        </div>
    );
};

const FunctionInvoker = () => {
    const { componentId, workerName } = useParams();
    const location = useLocation();
    const queryParams = new URLSearchParams(location.search);
    const functionName = queryParams.get("functionName");
    const exportName = functionName?.split(".")[0];
    const navigate = useNavigate();

    const {
        data: worker,
        isLoading,
        error: workerError,
    } = useWorker(componentId!, workerName!);
    const { data: component } = useComponent(componentId!);
    const [parameters, setParameters] = useState<Record<string, any>>({});
    const [result, setResult] = useState<any>(null);
    const [error, setError] = useState<string | null>(null);
    const [isInvoking, setIsInvoking] = useState(false);

    const invokeMutation = useMutation({
        mutationFn: async (params: any) => {
            const { data } = await apiClient.post(
                `/v1/components/${componentId}/workers/${workerName}/invoke-and-await?function=${functionName}`,
                params
            );
            return data;
        },
        onSuccess: (data: Object) => {
            setResult(data);
            toast.success('Function invoked successfully');
            setIsInvoking(false);
            setError(null);
        },
        onError: (error: Error) => {
            setError(error.message);
            setIsInvoking(false);
            toast.error(`Failed to invoke function: ${error.message}`);
        }
    });

    if (isLoading) {
        return (
            <div className='flex items-center justify-center h-64'>
                <div className='flex items-center gap-2 text-muted-foreground'>
                    <Loader2 className='animate-spin' size={20} />
                    <span>Loading function details...</span>
                </div>
            </div>
        );
    }

    if (workerError || !worker) {
        return (
            <div className='text-center py-12'>
                <AlertCircle className='mx-auto h-12 w-12 text-red-400 mb-4' />
                <h3 className='text-lg font-semibold mb-2'>Worker Not Found</h3>
                <p className='text-muted-foreground'>Failed to load worker details.</p>
                <button
                    onClick={() => navigate(-1)}
                    className='mt-6 text-blue-400 hover:text-blue-300 flex items-center gap-2 mx-auto'>
                    <ArrowLeft size={16} />
                    Go Back
                </button>
            </div>
        );
    }

    const exportDef = component?.metadata?.exports.find(
        (e) => e.name === exportName
    );
    const functionDef = exportDef?.functions.find(
        (f) =>
            f.name === functionName?.split(".")[1].replace("{", "").replace("}", "")
    );

    if (!functionDef) {
        return (
            <div className='text-center py-12'>
                <AlertCircle className='mx-auto h-12 w-12 text-red-400 mb-4' />
                <h3 className='text-lg font-semibold mb-2'>Function Not Found</h3>
                <p className='text-muted-foreground'>
                    The specified function could not be found.
                </p>
                <button
                    onClick={() => navigate(-1)}
                    className='mt-6 text-blue-400 hover:text-blue-300 flex items-center gap-2 mx-auto'>
                    <ArrowLeft size={16} />
                    Go Back
                </button>
            </div>
        );
    }

    const handleParameterChange = (path: string, value: any) => {
        const updateNestedValue = (
            obj: any,
            pathArray: string[],
            value: any
        ): any => {
            const [current, ...rest] = pathArray;
            if (rest.length === 0) {
                return { ...obj, [current]: value };
            }
            return {
                ...obj,
                [current]: updateNestedValue(obj[current] || {}, rest, value),
            };
        };

        setParameters((prev) => updateNestedValue(prev, path.split("."), value));
    };

    const formatParamsForAPI = () => {
        return {
            params: functionDef.parameters.map((param) => ({
                typ: param.typ,
                value: parameters[param.name],
            })),
        };
    };

    const handleInvoke = async () => {
        setIsInvoking(true);
        setError(null);
        setResult(null);
        // invokeWorkerMutation.mutate(
        //     componentId!,
        //     workerName!,
        //     functionName,
        //     formatParamsForAPI()
        // );
        invokeMutation.mutate(formatParamsForAPI());
    };

    return (
        <div className='max-w-4xl mx-auto space-y-6'>
            {/* Header */}
            <div className='bg-card/80 rounded-lg p-6'>
                <div className='flex items-center gap-4'>
                    <Link
                        to={`/components/${componentId}/workers/${workerName}`}
                        className='p-2 text-muted-foreground hover:text-gray-300 rounded-lg hover:bg-gray-700/60'>
                        <ArrowLeft size={20} />
                    </Link>
                    <div>
                        <h1 className='text-2xl font-bold flex items-center gap-2'>
                            <SquareFunction className='text-blue-400' size={24} />
                            {functionName}
                        </h1>
                        <div className='text-muted-foreground mt-1'>Export: {exportName}</div>
                    </div>
                </div>
            </div>

            {/* Parameters */}
            <div className='bg-card/80 rounded-lg p-6'>
                <h2 className='text-lg font-semibold flex items-center gap-2 mb-4'>
                    <Terminal className='text-muted-foreground' size={20} />
                    Parameters
                </h2>

                <div className='space-y-4'>
                    {functionDef.parameters.map((param) => (
                        <RecursiveParameterInput
                            key={param.name}
                            name={param.name}
                            typeDef={param.typ}
                            value={parameters[param.name]}
                            onChange={handleParameterChange}
                        />
                    ))}

                    {functionDef.parameters.length === 0 && (
                        <div className='text-center py-4 text-muted-foreground'>
                            This function takes no parameters
                        </div>
                    )}
                </div>

                <div className='mt-6 flex justify-end'>
                    <button
                        onClick={handleInvoke}
                        disabled={isInvoking || !!error}
                        className='flex items-center gap-2 px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600 
                     transition-colors disabled:opacity-50'>
                        {isInvoking ? (
                            <>
                                <Loader2 size={16} className='animate-spin' />
                                Invoking...
                            </>
                        ) : (
                            <>
                                <Play size={16} />
                                Invoke Function
                            </>
                        )}
                    </button>
                </div>
            </div>

            {/* Results */}
            {(result || error) && (
                <div className={`bg-card/80 border rounded-lg p-6 ${error ? 'border-destructive/20' : 'border-border/10'
                    }`}>
                    <h2 className='text-lg font-semibold flex items-center gap-2 mb-4'>
                        <Code2 className={error ? 'text-destructive' : 'text-primary'} size={20} />
                        {error ? 'Error' : 'Result'}
                    </h2>

                    <pre className='bg-card/60 rounded-lg p-4 font-mono text-sm overflow-auto'>
                        {error || JSON.stringify(result, null, 2)}
                    </pre>
                </div>
            )}
        </div>
    );
};

export default FunctionInvoker;
