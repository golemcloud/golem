import {Search} from "lucide-react";
import {Input} from "@/components/ui/input";
import {Table, TableBody, TableCell, TableHead, TableHeader, TableRow,} from "@/components/ui/table";
import {Select, SelectContent, SelectItem, SelectTrigger, SelectValue,} from "@/components/ui/select";
import {useEffect, useState} from "react";
import {API} from "@/service";
import {useParams} from "react-router-dom";
import {ComponentExportFunction, ComponentList, Export, Typ,} from "@/types/component.ts";
import {calculateExportFunctions} from "@/lib/utils";


export interface ExportResult {
    package: string,
    function_name: string,
    parameter: string,
    return: string
}

function parseType(typ: Typ) {
    if (!typ) return "null";

    if (typ.type === "Str") return "string";
    if (typ.type === "U64") return "number";
    if (typ.type === "Bool") return "boolean";
    if (typ.type === "Option") return `(${parseType(typ.inner!)} | null)`;
    if (typ.type === "List") return `${parseType(typ.inner!)}[]`;

    if (typ.type === "Record") {
        return `{\n  ${(typ.fields || []).map(field => `${field.name}: ${parseType(field.typ)}`).join(",\n  ")}\n}`;
    }

    if (typ.type === "Result") {
        return `result<\n  ${parseType(typ.ok!)},\n  ${parseEnum(typ.err!)}\n>`;
    }

    return "unknown";
}

function parseEnum(enumType: Typ) {
    if (!enumType || enumType.type !== "Enum") return "unknown";
    return enumType.cases!.map(c => `'${c}'`).join(" | ");
}

function generateFunctionInterfaces(data: Export[]) {
    const interfaces: ExportResult[] = [];

    data.forEach(instance => {
        instance.functions.forEach(func => {
            const functionName = func.name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()); // Convert kebab-case to camelCase

            const parameters = func.parameters.length > 0
                ? `{\n  ${func.parameters.map(param => `${param.name}: ${parseType(param.typ)}`).join(",\n  ")}\n}`
                : "()";

            const returnType = parseType(func.results[0]?.typ) || "void";

            interfaces.push({
                package: instance.name,
                function_name: functionName,
                parameter: parameters,
                return: returnType
            });
        });
    });

    return interfaces;
}


function parseTypeV1(typ: Typ) {
    if (!typ) return "null";

    if (typ.type === "Str") return `<span class="text-purple-800">string</span>`;
    if (typ.type === "U64") return `<span class="text-purple-800">number</span>`;
    if (typ.type === "Bool") return `<span class="text-purple-800">boolean</span>`;
    if (typ.type === "Option") return `<span class="text-purple-800">${parseTypeV1(typ.inner!)}</span> <span className="text-green-700">or</span> null`;
    if (typ.type === "List") return `${parseTypeV1(typ.inner!)}[]`;

    if (typ.type === "Record") {
        const fields = (typ.fields || []).map(field => `<span class="text-yellow-600">${field.name}</span>: ${parseTypeV1(field.typ)}`);
        return `{ ${fields.join(", ")} }`;
    }

    if (typ.type === "Result") {
        return `${parseTypeV1(typ.ok!)}  <span class="text-green-700">or</span>  ${parseEnumV1(typ.err!)}`;
    }

    return "unknown";
}

function parseEnumV1(enumType: Typ) {
    if (!enumType || enumType.type !== "Enum") return "unknown";
    return enumType.cases!.map(c => `'${c}'`).join(" | ");
}

function generateFunctionInterfacesV1(data: Export[]) {
    const interfaces: ExportResult[] = [];

    data.forEach(instance => {
        instance.functions.forEach(func => {
            const functionName = func.name.replace(/-([a-z])/g, (_, letter: string) => letter.toUpperCase()); // Convert kebab-case to camelCase

            const parameters = func.parameters.map(param => `<span class="text-yellow-600">${param.name}</span>: ${parseTypeV1(param.typ)}`).join(", <br/> ");

            const returnType = parseTypeV1(func.results[0]?.typ) || "void";

            interfaces.push({
                package: instance.name,
                function_name: functionName,
                parameter: parameters,
                return: returnType
            });
        });
    });

    return interfaces;
}


export default function Exports() {
    const {componentId = ""} = useParams();
    const [component, setComponent] = useState<ComponentList>({});
    const [versionList, setVersionList] = useState([] as number[]);
    const [versionChange, setVersionChange] = useState(0 as number);
    const [functions, setFunctions] = useState([] as ComponentExportFunction[]);
    const [result, setResult] = useState<ExportResult[]>([]);

    useEffect(() => {
        if (componentId) {
            API.getComponentByIdAsKey().then((response) => {
                setVersionList(response[componentId].versionList || []);
                setVersionChange(
                    response[componentId].versionList?.[
                    response[componentId].versionList?.length - 1
                        ] || 0
                );
                setComponent(response[componentId]);

                const componentDetails = component.versions?.find(
                    (data) => data.versionedComponentId?.version === versionChange
                );
                if (componentDetails) {
                    console.log(componentDetails.metadata?.exports);
                    const exports: ExportResult[] = generateFunctionInterfacesV1(componentDetails.metadata?.exports || []);
                    setResult(exports);
                    // const functions =
                    //     componentDetails.metadata?.exports.reduce(
                    //         (acc: ComponentExportFunction[], curr: Export) => {
                    //             const updatedFunctions = curr.functions.map(
                    //                 (func: ComponentExportFunction) => ({
                    //                     ...func,
                    //                     exportName: curr.name,
                    //                 })
                    //             );
                    //
                    //             return acc.concat(updatedFunctions);
                    //         },
                    //         []
                    //     ) || [];
                    // setFunctions(functions);
                }
            });
        }

    }, [componentId, versionChange]);

    const handleVersionChange = (version: number) => {
        setVersionChange(version);
    };

    const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
        const value = e.target.value;

        const searchResult = calculateExportFunctions(
            component.versions?.find(
                (data) => data.versionedComponentId?.version === versionChange
            )?.metadata?.exports || []
        ).filter((fn: ComponentExportFunction) => {
            return fn.name.includes(value);
        });
        setFunctions(searchResult || ([] as ComponentExportFunction[]));
    };

    return (
        <div className="flex">
            <div className="flex-1 p-8">
                <div className="p-6 max-w-7xl mx-auto space-y-6">
                    <div className="flex justify-between items-center">
                        <h1 className="text-2xl font-bold">Exports</h1>
                    </div>
                    <div className="flex items-center justify-between gap-10">
                        <div className="relative flex-1 max-full">
                            <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground"/>
                            <Input
                                placeholder="Search functions..."
                                className="pl-9"
                                onChange={(e) => handleSearch(e)}
                            />
                        </div>
                        {versionList.length > 0 && (
                            <Select
                                defaultValue={versionChange.toString()}
                                onValueChange={(version) => handleVersionChange(+version)}
                            >
                                <SelectTrigger className="w-[80px]">
                                    <SelectValue> v{versionChange} </SelectValue>
                                </SelectTrigger>
                                <SelectContent>
                                    {versionList.map((version: number) => (
                                        <SelectItem key={version} value={String(version)}>
                                            v{version}
                                        </SelectItem>
                                    ))}
                                </SelectContent>
                            </Select>
                        )}
                    </div>

                    <div className="border rounded-lg">
                        <Table>
                            <TableHeader>
                                <TableRow>
                                    <TableHead className="w-[250px]">Package</TableHead>
                                    <TableHead className="w-[200px]">Function</TableHead>
                                    {/*<TableHead className="w-[300px]">Parameters</TableHead>*/}
                                    {/*<TableHead>Return Value</TableHead>*/}
                                </TableRow>
                            </TableHeader>
                            <TableBody>
                                {result?.length > 0 ? (
                                    result.map((fn: ExportResult) => (
                                        <TableRow key={fn.function_name}>
                                            <TableCell className="font-mono text-sm">
                                                {fn.package}
                                            </TableCell>
                                            <TableCell className="font-mono text-sm">
                                                <span>{fn.function_name}</span>
                                                (
                                                <span dangerouslySetInnerHTML={{__html: fn.parameter}}/>
                                                ) {"=>"} <span dangerouslySetInnerHTML={{__html: fn.return}}/>
                                            </TableCell>
                                            {/*<TableCell className="font-mono text-sm">*/}
                                            {/*    /!*{fn.parameter})*!/*/}

                                            {/*</TableCell>*/}
                                            {/*<TableCell className="font-mono text-sm">*/}
                                            {/*    <div dangerouslySetInnerHTML={{__html: fn.return}}/>*/}
                                            {/*</TableCell>*/}
                                        </TableRow>
                                    ))
                                ) : (
                                    <div className="p-4 align-center grid">
                                        No exports found.
                                    </div>
                                )}
                            </TableBody>
                        </Table>
                    </div>
                </div>
            </div>
        </div>
    );
}
