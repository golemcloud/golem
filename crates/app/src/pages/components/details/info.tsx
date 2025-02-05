import {useParams} from "react-router-dom";

import {Button} from "@/components/ui/button";
import {Card, CardContent, CardDescription, CardHeader, CardTitle,} from "@/components/ui/card";
import {Select, SelectContent, SelectItem, SelectTrigger, SelectValue,} from "@/components/ui/select";
import {Download} from "lucide-react";
import {writeFile} from "@tauri-apps/plugin-fs";
import {BaseDirectory} from "@tauri-apps/api/path";
import {useEffect, useState} from "react";
import {ComponentList} from "@/types/component";
import {API} from "@/service";
import {formatRelativeTime} from "@/lib/utils";
import {toast} from "@/hooks/use-toast";

export default function ComponentInfo() {
    const {componentId = ""} = useParams();
    const [componentList, setComponentList] = useState<{
        [key: string]: ComponentList;
    }>({});
    const [versionList, setVersionList] = useState([] as number[]);
    const [versionChange, setVersionChange] = useState(0 as number);

    useEffect(() => {
        if (componentId) {
            API.getComponentByIdAsKey().then((response) => {
                const componentData = response[componentId];
                const versionList = componentData?.versionList || [];
                setVersionList(versionList);
                setComponentList(response);
                if (versionList.length > 0) {
                    setVersionChange(versionList[versionList.length - 1]);
                }
            });
        }
    }, [componentId]);

    const handleVersionChange = (version: number) => {
        setVersionChange(version);
    };

    async function downloadFile() {
        try {
            API.downloadComponent(componentId!, versionChange).then(
                async (response) => {
                    const blob = await response.blob();
                    const arrayBuffer = await blob.arrayBuffer();

                    // Specify the file name and path
                    const fileName = `${componentId}.wasm`;
                    await writeFile(fileName, new Uint8Array(arrayBuffer), {
                        baseDir: BaseDirectory.Download,
                    });
                    toast({
                        title: "File downloaded successfully",
                        duration: 3000,
                    });
                }
            );
        } catch (error) {
            console.error("Error downloading the file:", error);
        }
    }

    const componentDetails =
        componentList[componentId]?.versions?.[versionChange] || {};

    return (
        <div className="flex">
            <div className="flex-1 p-8">
                <Card className="w-full max-w-3xl mx-auto">
                    <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-7">
                        <div className="space-y-1.5">
                            <CardTitle className="text-2xl font-semibold">
                                Component Information
                            </CardTitle>
                            <CardDescription>
                                View metadata about this component
                            </CardDescription>
                        </div>
                        <div className="flex items-center gap-2">
                            {versionList.length > 0 && (
                                <Select
                                    defaultValue={versionChange.toString()}
                                    onValueChange={(version) => handleVersionChange(+version)}
                                >
                                    <SelectTrigger className="w-[80px]">
                                        <SelectValue> v{versionChange}</SelectValue>
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
                            <Button variant="outline" size="icon" onClick={downloadFile}>
                                <Download className="h-4 w-4"/>
                            </Button>
                        </div>
                    </CardHeader>
                    <CardContent>
                        <div className="space-y-4">
                            <div className="grid grid-cols-[180px,1fr] items-center gap-4 py-3 border-b">
                                <div className="text-sm font-medium text-muted-foreground">
                                    Component ID
                                </div>
                                <div className="font-mono text-sm">
                                    {componentDetails.versionedComponentId?.componentId}
                                </div>
                            </div>
                            <div className="grid grid-cols-[180px,1fr] items-center gap-4 py-3 border-b">
                                <div className="text-sm font-medium text-muted-foreground">
                                    Version
                                </div>
                                <div className="font-mono text-sm">{versionChange}</div>
                            </div>
                            <div className="grid grid-cols-[180px,1fr] items-center gap-4 py-3 border-b">
                                <div className="text-sm font-medium text-muted-foreground">
                                    Name
                                </div>
                                <div className="font-mono text-sm">
                                    {componentDetails.componentName}
                                </div>
                            </div>
                            <div className="grid grid-cols-[180px,1fr] items-center gap-4 py-3 border-b">
                                <div className="text-sm font-medium text-muted-foreground">
                                    Size
                                </div>
                                <div className="font-mono text-sm">
                                    {Math.round(
                                        (componentDetails?.componentSize || 0) / 1024
                                    )}{" "}
                                    KB
                                </div>
                            </div>
                            <div className="grid grid-cols-[180px,1fr] items-center gap-4 py-3">
                                <div className="text-sm font-medium text-muted-foreground">
                                    Created At
                                </div>
                                <div className="font-mono text-sm">
                                    {componentDetails.createdAt
                                        ? formatRelativeTime(componentDetails.createdAt)
                                        : "NA"}
                                </div>
                            </div>
                        </div>
                    </CardContent>
                </Card>
            </div>
        </div>
    );
}
