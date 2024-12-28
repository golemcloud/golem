import useSWR, { mutate } from "swr";
import { fetcher, getErrorMessage } from "../utils";
import { Plugin } from "@/types/api";
import { toast } from "react-toastify";
import { useParams } from "next/navigation";
import { useMemo } from "react";
const PULGIN_PATH = "?path=plugins";

export function useDeletePlugin() {
  const deletePlugin = async (name: string, version: string) => {
    try {
      const response = await fetcher(`${PULGIN_PATH}/${name}/${version}`, {
        method: "DELETE",
      });
      if (response.status !== 200) {
        const error = getErrorMessage(response);
        toast.error(`Plugin failed to delete due to: ${error}`);
        return { success: false, error };
      }
      mutate(PULGIN_PATH);
      toast.success("Plugin Deleted Successfully");
      return { success: true, data: response.data };
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
      if (response.status !== 200) {
        const error = getErrorMessage(response);
        toast.error(`Plugin failed to create due to: ${error}`);
        return { success: false, error };
      }
      mutate(PULGIN_PATH);
      toast.success("Plugin has successfully resumed");
      return { success: true, data: response.data };
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
  const { name, version } = useParams<{ name: string; version: string }>();
  let path = `${PULGIN_PATH}`;
  path = name ? `${path}/${name}` : path;
  path = name && version ? `${path}/${version}` : path;
  const { data, error: requestError, isLoading } = useSWR(path, fetcher);

  const error = useMemo(() => {
    if (!isLoading && data?.status !== 200) {
      return getErrorMessage(data);
    }
    return !isLoading ? getErrorMessage(requestError) : "";
  }, [isLoading, requestError, data]);
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
    error,
    getPluginByName,
    isLoading,
  };
}
