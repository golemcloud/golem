"use client";

import ErrorBoundary from "@/components/error/error-boundary";
import ApiDetails from "../../route-info";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { Loader } from "lucide-react";
import { useSearchParams } from "next/navigation";

const RouteDetails = ({defaultRouteId, defaultVersion, noRedirect}: {defaultRouteId?: string, defaultVersion?: string, noRedirect?: boolean}) => {
  const { routeId } = useCustomParam();
  const params = useSearchParams();
  const { apiId } = useCustomParam();
  const version = defaultVersion || params.get("version");
  const {
    isLoading,
    getApiDefintion,
    error: requestError,
  } = useApiDefinitions(apiId, version);
  const { data: apiDefinition, error } = (!isLoading && getApiDefintion()) || {};
  const [path, method] = decodeURIComponent(defaultRouteId || routeId).split('|');

  const route = apiDefinition?.routes.find((route) => {
    return route.method === method && route.path === path;
  });

  if (isLoading) {
    return <Loader />;
  }

  if (requestError || error) {
    return <ErrorBoundary message={requestError || error} />;
  }

  return (
    apiDefinition && route
      ? <ApiDetails route={route} version={apiDefinition?.version} noRedirect={noRedirect} isDraft={apiDefinition?.draft} />
      : <>No route found!</>
  );
};

export default RouteDetails;
