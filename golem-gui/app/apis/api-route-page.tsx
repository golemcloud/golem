import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { RouteList } from "./route-list";

export default function RoutePage({
  apiId,
  version,
  limit,
}: {
  apiId: string;
  version?: string | null;
  limit?: number;
}) {
  const {
    isLoading,
    getApiDefintion,
    error: requestError,
  } = useApiDefinitions(apiId, version);
  const { data: apiDefintion, error } = (!isLoading && getApiDefintion()) || {};

  return (
    <>
      <RouteList
        isLoading={isLoading}
        apiDefintion={apiDefintion}
        limit={limit}
        error={requestError || error}
      />
    </>
  );
}
