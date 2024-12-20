import useSWR from "swr";
import { fetcher } from "@/lib/utils";

export function useComponents () {
    const { data: componentData, isLoading } = useSWR("?path=components", fetcher);
    return {
      data: componentData,
      isLoading,
    }
  }