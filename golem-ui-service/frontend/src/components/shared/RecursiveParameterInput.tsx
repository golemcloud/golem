import { MinusCircle, PlusCircle } from 'lucide-react';

const TypeBadge = ({ type }: { type: string }) => (
    <span className="px-2 py-0.5 rounded-full text-xs bg-blue-500/10 text-blue-400 font-mono">
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
    inner?: TypeDefinition;
}

const createEmptyValue = (typeDef: TypeDefinition): unknown => {
    switch (typeDef.type) {
        case "Record":
            const record: Record<string, unknown> = {};
            typeDef.fields?.forEach(field => {
                record[field.name] = createEmptyValue(field.typ);
            });
            return record;

        case "List":
            return [];

        case "Option":
            return null;

        case "Str":
            return "";

        default:
            return null;
    }
};

const RecursiveParameterInput = ({
    name,
    typeDef,
    value,
    onChange,
    path = "",
}: {
    name: string;
    typeDef: TypeDefinition;
    value: unknown;
    onChange: (path: string, value: unknown) => void;
    path?: string;
}) => {
    const currentPath = path ? `${path}.${name}` : name;

    const handleValueChange = (newValue: unknown) => {
        onChange(currentPath, newValue);
    };

    const renderInput = () => {
        switch (typeDef.type) {
            case "Record":
                return (
                    <div className="space-y-4 bg-card/60 p-4 rounded-lg border border-border/20">
                        {typeDef.fields?.map((field) => (
                            <div key={field.name}>
                                <RecursiveParameterInput
                                    name={field.name}
                                    typeDef={field.typ}
                                    value={value?.[field.name]}
                                    onChange={(fieldPath, fieldValue) => {
                                        const newValue = { ...(value || {}) };
                                        newValue[field.name] = fieldValue;
                                        handleValueChange(newValue);
                                    }}
                                    path={currentPath}
                                />
                            </div>
                        ))}
                    </div>
                );

            case "Variant":
                return (
                    <div className="space-y-4">
                        <select
                            className="w-full bg-card/70 border border-border/20 rounded-lg p-2 text-sm"
                            value={(value as { type: string })?.type || ""}
                            onChange={(e) => {
                                const selectedCase = typeDef.cases?.find(
                                    (c) => c.name === e.target.value
                                );
                                if (selectedCase) {
                                    handleValueChange({
                                        type: e.target.value,
                                        value: createEmptyValue(selectedCase.typ)
                                    });
                                } else {
                                    handleValueChange(null);
                                }
                            }}
                        >
                            <option value="">Select type...</option>
                            {typeDef.cases?.map((caseItem) => (
                                <option key={caseItem.name} value={caseItem.name}>
                                    {caseItem.name}
                                </option>
                            ))}
                        </select>
                        {(value as { type: string; value: unknown })?.type && (
                            <div className="pl-4 border-l-2 border-border/20">
                                <RecursiveParameterInput
                                    name="value"
                                    typeDef={
                                        typeDef.cases!.find(
                                            (c) => c.name === (value as { type: string }).type
                                        )!.typ
                                    }
                                    value={(value as { value: unknown }).value}
                                    onChange={(_, newValue) =>
                                        handleValueChange({
                                            type: (value as { type: string }).type,
                                            value: newValue,
                                        })
                                    }
                                    path={currentPath}
                                />
                            </div>
                        )}
                    </div>
                );

            case "List":
                return (
                    <div className="space-y-2">
                        {Array.isArray(value) && value.length > 0 ? (
                            <div className="space-y-2">
                                {value.map((item, index) => (
                                    <div key={index} className="flex gap-2 items-start">
                                        <div className="flex-1">
                                            <RecursiveParameterInput
                                                name={index.toString()}
                                                typeDef={typeDef.inner!}
                                                value={item}
                                                onChange={(_, newValue) => {
                                                    const newArray = [...(value || [])];
                                                    newArray[index] = newValue;
                                                    handleValueChange(newArray);
                                                }}
                                                path={currentPath}
                                            />
                                        </div>
                                        <button
                                            onClick={() => {
                                                const newArray = value.filter((_, i) => i !== index);
                                                handleValueChange(newArray);
                                            }}
                                            className="p-2 text-destructive hover:text-destructive/80 rounded-lg"
                                            title="Remove item"
                                        >
                                            <MinusCircle size={16} />
                                        </button>
                                    </div>
                                ))}
                            </div>
                        ) : (
                            <div className="text-center py-2 text-muted-foreground text-sm">
                                No items added
                            </div>
                        )}
                        <button
                            onClick={() => {
                                const newItem = createEmptyValue(typeDef.inner!);
                                handleValueChange([...(value || []), newItem]);
                            }}
                            className="flex items-center gap-1 text-primary hover:text-primary/80 text-sm"
                        >
                            <PlusCircle size={16} />
                            Add Item
                        </button>
                    </div>
                );

            case "Option":
                return (
                    <div className="space-y-2">
                        <div className="flex items-center gap-2">
                            <input
                                type="checkbox"
                                checked={value !== null && value !== undefined}
                                onChange={(e) => handleValueChange(e.target.checked ? "" : null)}
                                className="rounded border-border/20"
                            />
                            <span className="text-sm text-muted-foreground">Optional value</span>
                        </div>
                        {value !== null && value !== undefined && (
                            <RecursiveParameterInput
                                name={name}
                                typeDef={typeDef.inner!}
                                value={value}
                                onChange={(_, newValue) => handleValueChange(newValue)}
                                path={currentPath}
                            />
                        )}
                    </div>
                );

            case "Str":
                return (
                    <input
                        type="text"
                        className="w-full bg-card/80 border border-border/20 rounded-lg p-2 font-mono text-sm"
                        placeholder={`Enter ${name}...`}
                        value={(value as string) || ""}
                        onChange={(e) => handleValueChange(e.target.value)}
                    />
                );

            case "Bool":
                return (
                    <select
                        className="w-full bg-card/90 border border-foreground rounded-lg p-2 text-sm"
                        value={value?.toString()}
                        onChange={(e) => handleValueChange(e.target.value === "true")}
                    >
                        <option value="true">true</option>
                        <option value="false">false</option>
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
                        className="w-full bg-card/80 border border-foreground rounded-lg p-2 font-mono text-sm"
                        placeholder={`Enter ${name}...`}
                        value={(value as string) || ""}
                        onChange={(e) => handleValueChange(e.target.value)}
                        step={typeDef.type.toLowerCase().startsWith("f") ? "0.01" : "1"}
                    />
                );
        }
    }

    return (
        <div className="space-y-2">
            <label className="flex items-center gap-2 text-sm font-medium">
                {name}
                <TypeBadge type={typeDef.type} />
            </label>
            {renderInput()}
        </div>
    );
};

export default RecursiveParameterInput;