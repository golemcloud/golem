import { Button2, AddIcon, CustomModal } from "@/components/imports";
import { Box, InputAdornment, TextField } from "@mui/material";
import SearchIcon from "@mui/icons-material/Search";
import { Plugin } from "@/types/api";
import React, {useMemo, useState, useCallback } from "react";
import PluginInstallForm, { PluginUninstallForm } from "../../install-plugin-form";
import useComponents, { useInstallPlugins } from "@/lib/hooks/use-component";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import GenericTable from "@/components/ui/generic-table";

interface Column<T> {
  key: string;
  label: string;
  accessor: (item: T) => React.ReactNode;
  align?: "left" | "center" | "right";
}
export default function InstallPlugin() {
  const [searchQuery, setSearchQuery] = useState("");
  const { compId } = useCustomParam();
  const { components } = useComponents(compId, "latest");
  const [selectedPlugin, setSelectedPlugin] = useState<Plugin | null>(null);
  console.log("components", components);
  const [latestComponent] = components;

  const finalversion = latestComponent?.versionedComponentId?.version;
  const { installedPlugins } = useInstallPlugins(compId, Number(finalversion));

  const [open, setOpen] = useState(false);
  const [open1, setOpen1] = useState(false);

  const checkForMatch = useCallback(
    (plugin: Plugin) => {
      if (!searchQuery || searchQuery.length <= 2) {
        return true;
      }
      return plugin.name.toLowerCase().includes(searchQuery.toLowerCase());
    },
    [searchQuery]
  );

  const finalPlugins = useMemo(() => {
    if (!installedPlugins) return [];
    return installedPlugins.filter(checkForMatch);
  }, [installedPlugins, checkForMatch]);

  const handleClose = () => setOpen(false);
  const handleClose1 = () => setOpen1(false);
  console.log("finalPlugins", finalPlugins);
  console.log("version", finalversion); 
  const columns: Column<Plugin>[] = [
    {
      key: "name",
      label: "Plugin Name",
      accessor: (plugin: Plugin) => plugin.name,
      align: "left",
    },
    {
      key: "version",
      label: "Version",
      accessor: (plugin: Plugin) => plugin.version,
      align: "center",
    },
    {
      key: "priority",
      label: "Priority",
      // @ts-expect-error - The structure of `plugin` is not fully typed yet
      accessor: (plugin: Plugin) => plugin.priority,
      align: "right",
    },
  ];
  
  const handleRowClick = (plugin: Plugin) => {
    setSelectedPlugin(plugin);
    setOpen1(true);
  };
  

  return (
    <Box display="flex" flexDirection="column" gap={2}>
      <Box display="flex" justifyContent="space-between" alignItems="center">
        <TextField
          placeholder="Search Plugins..."
          variant="outlined"
          size="small"
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          InputProps={{
            startAdornment: (
              <InputAdornment position="start">
                <SearchIcon sx={{ color: "grey.500" }} />
              </InputAdornment>
            ),
          }}
          className="flex-1"
        />
        <Button2 className="ml-2" variant="default" endIcon={<AddIcon />} size="md" onClick={() => setOpen(true)}>
          Install Plugin
        </Button2>
      </Box>

      <Box>
        <GenericTable onRowClick={handleRowClick} data={finalPlugins} columns={columns} />
      </Box>
      <CustomModal open={open1} onClose={handleClose1} heading={"Uninstall  Plugin"}>
        <PluginUninstallForm plugin={selectedPlugin} onSuccess={handleClose1} />
      </CustomModal>
      <CustomModal open={open} onClose={handleClose} heading={"Install New Plugin"}>
        <PluginInstallForm onSuccess={handleClose} />
      </CustomModal>
    </Box>
  );
}



