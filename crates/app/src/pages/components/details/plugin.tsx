import {Search, Trash2} from "lucide-react";
import {Input} from "@/components/ui/input";
import {Table, TableBody, TableCell, TableHead, TableHeader, TableRow} from "@/components/ui/table";
import {Select, SelectContent, SelectItem, SelectTrigger, SelectValue} from "@/components/ui/select";
import {useEffect, useState} from "react";
import {API} from "@/service";
import {useParams} from "react-router-dom";
import {Component, ComponentList, InstalledPlugin} from "@/types/component.ts";
import {toast} from "@/hooks/use-toast.ts";

export default function Plugins() {
    const {componentId = ""} = useParams();
    const [component, setComponent] = useState<ComponentList>({} as ComponentList);
    const [plugins, setPlugins] = useState<InstalledPlugin[]>([]);
    const [filteredPlugins, setFilteredPlugins] = useState<InstalledPlugin[]>([]);
    const [versionList, setVersionList] = useState<number[]>([]);
    const [versionChange, setVersionChange] = useState(0);

    // Fetch component versions & plugins on mount
    useEffect(() => {
        if (!componentId) return;
        refreshComponent();

    }, [componentId]);

    const refreshComponent = () => {
        API.getComponentByIdAsKey().then((response) => {
            setComponent(response[componentId]);
            const data = response[componentId];
            const versionList = data.versionList || [];
            setVersionList(versionList)
            if (versionList.length > 0) {
                handleVersionChange(versionList[versionList.length - 1]);
            }
        });
    }

    const handleVersionChange = (version: number, fetchedComponent?: Component) => {
        setVersionChange(version);

        const fetchComponent = fetchedComponent
            ? Promise.resolve(fetchedComponent)
            : API.getComponentByIdAndVersion(componentId, version);

        fetchComponent.then((response) => {
            setPlugins(response.installedPlugins || []);
            setFilteredPlugins(response.installedPlugins || []);
        });
    };

    // Handle search input
    const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
        const query = e.target.value.trim().toLowerCase();
        setFilteredPlugins(
            plugins.filter((plugin) => plugin.name.toLowerCase().includes(query))
        );
    };

    // Handle delete action
    const handleDeletePlugin = (pluginId: string) => {
        // console.log(pluginId + " " + componentId, versionChange);
        const versionList = component.versionList || [];
        const latestVersion = versionList[versionList.length - 1];
        if (latestVersion === versionChange) {
            API.deletePluginToComponent(componentId, pluginId).then((_) => {
                toast({
                    title: "Plugin deleted successfully",
                    description: "Plugin has been deleted successfully. Please check the latest version of the component.",
                    duration: 3000,
                });
                refreshComponent();
            });
        }
    };

    return (
        <div className="flex">
            <div className="flex-1 p-8">
                <div className="p-6 max-w-7xl mx-auto space-y-6">
                    <div className="flex items-center justify-between gap-10">
                        {/* Search Input */}
                        <div className="relative flex-1 max-full">
                            <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground"/>
                            <Input placeholder="Search plugins..." className="pl-9" onChange={handleSearch}/>
                        </div>

                        {/* Version Selector */}
                        {versionList.length > 0 && (
                            <Select defaultValue={String(versionChange)}
                                    onValueChange={(value) => handleVersionChange(Number(value))}>
                                <SelectTrigger className="w-[80px]">
                                    <SelectValue>v{versionChange}</SelectValue>
                                </SelectTrigger>
                                <SelectContent>
                                    {versionList.map((version) => (
                                        <SelectItem key={version} value={String(version)}>v{version}</SelectItem>
                                    ))}
                                </SelectContent>
                            </Select>
                        )}
                    </div>

                    {/* Plugins Table */}
                    <div className="border rounded-lg">
                        <Table>
                            <TableHeader>
                                <TableRow>
                                    <TableHead className="w-1/4">Name</TableHead>
                                    <TableHead className="w-1/4">Version</TableHead>
                                    <TableHead className="w-1/4">Priority</TableHead>
                                    <TableHead className="w-1/4 text-right">Actions</TableHead>
                                </TableRow>
                            </TableHeader>
                            <TableBody>
                                {filteredPlugins.length > 0 ? (
                                    filteredPlugins.map((plugin) => (
                                        <TableRow key={plugin.id} className="group">
                                            <TableCell className="font-mono text-sm">{plugin.name}</TableCell>
                                            <TableCell className="font-mono text-sm">{plugin.version}</TableCell>
                                            <TableCell className="font-mono text-sm">{plugin.priority}</TableCell>
                                            <TableCell className="w-1/4 text-right">
                                                <button
                                                    onClick={() => handleDeletePlugin(plugin.id)}
                                                    className="opacity-0 group-hover:opacity-100 transition-opacity duration-200 cursor-pointer"
                                                >
                                                    <Trash2 className="h-5 w-5 text-red-500 hover:text-red-700"/>
                                                </button>
                                            </TableCell>
                                        </TableRow>
                                    ))
                                ) : (
                                    <tr>
                                        <td colSpan={4} className="p-4 text-center text-gray-500">No plugins found.</td>
                                    </tr>
                                )}
                            </TableBody>
                        </Table>
                    </div>
                </div>
            </div>
        </div>
    );
}