import { Search, Trash2 } from "lucide-react";
import { Input } from "@/components/ui/input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useEffect, useState } from "react";
import { API } from "@/service";
import { useParams } from "react-router-dom";
import { ComponentList, InstalledPlugin } from "@/types/component.ts";
import { toast } from "@/hooks/use-toast.ts";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";

export default function Plugins() {
  const { componentId = "", appId } = useParams();
  const [component, setComponent] = useState<ComponentList>(
    {} as ComponentList,
  );
  const [plugins, setPlugins] = useState<InstalledPlugin[]>([]);
  const [filteredPlugins, setFilteredPlugins] = useState<InstalledPlugin[]>([]);
  const [versionList, setVersionList] = useState<number[]>([]);
  const [versionChange, setVersionChange] = useState(0);
  const [isDialogOpen, setIsDialogOpen] = useState(false);
  const [isDeleteDialogOpen, setIsDeleteDialogOpen] = useState(false);
  const [pluginToDelete, setPluginToDelete] = useState<InstalledPlugin | null>(
    null,
  );
  const [newPlugin, setNewPlugin] = useState({
    name: "",
    priority: 1,
    version: "",
  });
  const [availablePlugin, setAvailablePlugin] = useState<
    Record<string, string[]>
  >({});

  useEffect(() => {
    const fetchPlugins = async () => {
      try {
        const plugins = await API.pluginService.getPlugins(appId!);
        const pluginMap: Record<string, string[]> = {};
        plugins.forEach(({ name, versions }) => {
          if (!pluginMap[name]) {
            pluginMap[name] = [];
          }
          // Extract version strings from the versions array
          versions.forEach(plugin => {
            if (!pluginMap[name]?.includes(plugin.version)) {
              pluginMap[name]?.push(plugin.version);
            }
          });
        });
        setAvailablePlugin(pluginMap);
      } catch (error) {
        toast({
          title: "Failed to fetch plugins",
          description: `An error occurred while fetching the plugin list. ${error}`,
          variant: "destructive",
          duration: 5000,
        });
      }
    };

    fetchPlugins();
    if (!componentId) return;
    refreshComponent();
  }, []);

  const refreshComponent = () => {
    API.componentService.getComponentByIdAsKey(appId!).then(response => {
      setComponent(response[componentId]!);
      const data = response[componentId];
      const versionList = data?.versionList || [];
      setVersionList(versionList);
      if (versionList.length > 0) {
        handleVersionChange(versionList[versionList.length - 1]!);
      }
    });
  };

  const handleVersionChange = (version: number) => {
    setVersionChange(version);

    // Fetch installed plugins using the CLI command
    API.componentService
      .getInstalledPlugins(appId!, componentId)
      .then(installedPlugins => {
        setPlugins(installedPlugins);
        setFilteredPlugins(installedPlugins);
      })
      .catch(error => {
        console.error("Failed to fetch installed plugins:", error);
        toast({
          title: "Failed to fetch installed plugins",
          description: `An error occurred while fetching installed plugins. ${error}`,
          variant: "destructive",
          duration: 5000,
        });
        // Fallback to empty array
        setPlugins([]);
        setFilteredPlugins([]);
      });
  };

  const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
    const query = e.target.value.trim().toLowerCase();
    setFilteredPlugins(
      plugins.filter(plugin => plugin.name.toLowerCase().includes(query)),
    );
  };

  // Handle delete action - opens confirmation dialog
  const handleDeletePlugin = (pluginId: string) => {
    const versionList = component.versionList || [];
    const latestVersion = versionList[versionList.length - 1];
    if (latestVersion === versionChange) {
      // Find the plugin details by ID
      const plugin = plugins.find(p => p.id === pluginId);
      if (plugin) {
        setPluginToDelete(plugin);
        setIsDeleteDialogOpen(true);
      } else {
        toast({
          title: "Plugin not found",
          description: "Could not find the plugin to delete.",
          variant: "destructive",
          duration: 5000,
        });
      }
    }
  };

  // Confirm delete action - actually deletes the plugin
  const confirmDeletePlugin = () => {
    if (pluginToDelete) {
      API.componentService
        .deletePluginToComponentWithApp(
          appId!,
          componentId,
          pluginToDelete.id, // Use installation ID
        )
        .then(() => {
          toast({
            title: "Plugin deleted successfully",
            description:
              "Plugin has been deleted successfully. Please check the latest version of the component.",
            duration: 3000,
          });
          refreshComponent();
          setIsDeleteDialogOpen(false);
          setPluginToDelete(null);
        })
        .catch(error => {
          toast({
            title: "Failed to delete plugin",
            description: `An error occurred while deleting the plugin. ${error}`,
            variant: "destructive",
            duration: 5000,
          });
        });
    }
  };

  const handleAddPlugin = () => {
    const pluginData = {
      name: newPlugin.name,
      priority: newPlugin.priority,
      version: newPlugin.version,
      parameters: {},
    };
    API.componentService
      .addPluginToComponentWithApp(appId!, componentId, pluginData)
      .then(() => {
        toast({
          title: "Plugin added successfully",
          description: "The new plugin has been added successfully.",
          duration: 3000,
        });
        refreshComponent();
        setIsDialogOpen(false);
        setNewPlugin({ name: "", priority: 1, version: "" });
      })
      .catch(error => {
        toast({
          title: "Failed to add plugin",
          description: `An error occurred while adding the plugin. ${error}`,
          variant: "destructive",
          duration: 5000,
        });
      });
  };

  return (
    <div className="container mx-auto p-6 space-y-6">
      <div className="flex items-center justify-between gap-4">
        <div className="relative flex-1">
          <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-gray-400" />
          <Input
            placeholder="Search plugins..."
            className="pl-10"
            onChange={handleSearch}
          />
        </div>
        {versionList.length > 0 && (
          <Select
            defaultValue={String(versionChange)}
            onValueChange={value => handleVersionChange(Number(value))}
          >
            <SelectTrigger className="w-[100px]">
              <SelectValue>v{versionChange}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {versionList.map(version => (
                <SelectItem key={version} value={String(version)}>
                  v{version}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        <Dialog open={isDialogOpen} onOpenChange={setIsDialogOpen}>
          <DialogTrigger asChild>
            <Button>Add Plugin</Button>
          </DialogTrigger>
          <DialogContent>
            <DialogTitle>Add New Plugin</DialogTitle>
            <DialogDescription>
              Enter the details of the new plugin you want to add.
            </DialogDescription>
            <div className="space-y-4">
              <div>
                <Label htmlFor="plugin-name">Plugin Name</Label>
                <Select
                  onValueChange={value => {
                    setNewPlugin({ ...newPlugin, name: value });
                  }}
                >
                  <SelectTrigger>
                    <SelectValue>
                      {newPlugin.name || "Select a plugin"}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {Object.keys(availablePlugin).length > 0 ? (
                      Object.keys(availablePlugin).map(plugin => (
                        <SelectItem key={plugin} value={plugin}>
                          {plugin}
                        </SelectItem>
                      ))
                    ) : (
                      <div className="text-center text-muted-foreground">
                        No plugins found.
                      </div>
                    )}
                  </SelectContent>
                </Select>
              </div>
              <div>
                <Label htmlFor="plugin-version">Version</Label>
                <Select
                  name="plugin-version"
                  disabled={!newPlugin.name}
                  onValueChange={value => {
                    setNewPlugin({ ...newPlugin, version: value });
                  }}
                >
                  <SelectTrigger>
                    <SelectValue>
                      {newPlugin.version || "Select a Plugin Version"}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {(availablePlugin[newPlugin.name || ""] || []).map(
                      version => (
                        <SelectItem key={version} value={version}>
                          {version}
                        </SelectItem>
                      ),
                    )}
                  </SelectContent>
                </Select>
              </div>
              <div>
                <Label htmlFor="plugin-priority">Priority</Label>
                <Input
                  id="plugin-priority"
                  placeholder="Priority"
                  value={newPlugin.priority}
                  onChange={e =>
                    setNewPlugin({
                      ...newPlugin,
                      priority: Number(e.target.value),
                    })
                  }
                />
              </div>
            </div>
            <div className="flex justify-end mt-4 gap-2">
              <DialogClose asChild>
                <Button variant="outline">Cancel</Button>
              </DialogClose>
              <Button onClick={handleAddPlugin}>Add</Button>
            </div>
          </DialogContent>
        </Dialog>

        {/* Delete Confirmation Dialog */}
        <Dialog open={isDeleteDialogOpen} onOpenChange={setIsDeleteDialogOpen}>
          <DialogContent>
            <DialogTitle>Confirm Plugin Deletion</DialogTitle>
            <DialogDescription>
              Are you sure you want to delete the plugin &quot;
              {pluginToDelete?.name}&quot; (version {pluginToDelete?.version})?
              This action cannot be undone.
            </DialogDescription>
            <div className="flex justify-end mt-4 gap-2">
              <DialogClose asChild>
                <Button
                  variant="outline"
                  onClick={() => setPluginToDelete(null)}
                >
                  Cancel
                </Button>
              </DialogClose>
              <Button variant="destructive" onClick={confirmDeletePlugin}>
                Delete Plugin
              </Button>
            </div>
          </DialogContent>
        </Dialog>
      </div>
      <div className="border rounded-lg shadow-sm">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Version</TableHead>
              <TableHead>Priority</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {filteredPlugins.length > 0 ? (
              filteredPlugins.map(plugin => (
                <TableRow key={plugin.id}>
                  <TableCell>{plugin.name}</TableCell>
                  <TableCell>{plugin.version}</TableCell>
                  <TableCell>{plugin.priority}</TableCell>
                  <TableCell className="text-right">
                    <button
                      onClick={() => handleDeletePlugin(plugin.id)}
                      className="text-red-500 hover:text-red-700"
                    >
                      <Trash2 className="h-5 w-5" />
                    </button>
                  </TableCell>
                </TableRow>
              ))
            ) : (
              <TableRow>
                <TableCell
                  colSpan={4}
                  className="text-center py-4 text-gray-500"
                >
                  No plugins found.
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
