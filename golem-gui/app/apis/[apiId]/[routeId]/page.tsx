"use client";

import ApiDetails from "@/components/route-info";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { useSearchParams } from "next/navigation";

const RouteDetails = () => {
  const { routeId } = useCustomParam();
  const params = useSearchParams();
  const { apiId } = useCustomParam();
  const version = params.get("version");
  const {
    isLoading,
    getApiDefintion,
    error: requestError,
  } = useApiDefinitions(apiId, version);
  const { data: apiDefinition, error } = (!isLoading && getApiDefintion()) || {};
  const [path, method] = decodeURIComponent(routeId).split('|');

  const route=apiDefinition?.routes.find((route)=>{ 
     return route.method==method && route.path==path;
  });

  return (
    route ? <ApiDetails route={route}/>: <>No route found!</>
  );
};

export default RouteDetails;
