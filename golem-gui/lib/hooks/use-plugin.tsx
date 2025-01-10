import useSWR, { mutate } from "swr";
import { fetcher } from "../utils";
import { Plugin } from "@/types/api";
import { toast } from "react-toastify";
import { useCustomParam } from "./use-custom-param";
const PULGIN_PATH = "v1/plugins";

export function useDeletePlugin() {
  const deletePlugin = async (name: string, version: string) => {
    try {
      const response = await fetcher(`${PULGIN_PATH}/${name}/${version}`, {
        method: "DELETE",
      });
      if (response.error) {
        toast.error(`Plugin failed to delete due to: ${response.error}`);
        return response;
      }
      mutate(PULGIN_PATH);
      toast.success("Plugin Deleted Successfully");
      return response;
    } catch (err) {
      console.error("Fialed to delete plugin due to", err);
      toast.error("Something went wrong!");
    }
  };
  return {
    deletePlugin,
  };
}

export function useAddPlugin() {
  const upsertPulgin = async (pulginData: Plugin) => {
    try {
      const response = await fetcher(`${PULGIN_PATH}`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
        },
        body: JSON.stringify(pulginData),
      });
      if (response.error) {
        toast.error(`Plugin failed to create due to: ${response.error}`);
        return response;
      }
      mutate(PULGIN_PATH);
      toast.success("Plugin Created successfully");
      return response;
    } catch (err) {
      console.error("Fialed to create plugin due to", err);
      toast.error("Something went wrong!");
    }
  };

  return {
    upsertPulgin,
  };
}

export default function usePlugins() {
  const { name, version } = useCustomParam();
  let path = `${PULGIN_PATH}`;
  path = name ? `${path}/${name}` : path;
  path = name && version ? `${path}/${version}` : path;
  const { data, error, isLoading } = useSWR(path, fetcher);

  const plugins = (data?.data || []) as Plugin[];

  const getPluginByName = (
    name: string
  ): { success: boolean; error?: string | null; data?: Plugin } => {
    const plugin = plugins?.find((plugin: Plugin) => plugin.name === name);

    if (!plugin) {
      return { success: false, error: "No Plugin found!" };
    }

    return {
      success: true,
      data: plugin,
    };
  };

  return {
    plugins,
    error: error || data?.error,
    getPluginByName,
    isLoading,
  };
}
