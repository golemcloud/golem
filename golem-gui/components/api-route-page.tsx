"use client";
import { Box, Typography, Stack, List } from "@mui/material";
import { Loader } from "lucide-react";
import { ApiDefinition, ApiRoute } from "../types/api";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import ErrorBoundary from "./erro-boundary";
import { useRouter } from "next/navigation";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { Button2 } from "./ui/button";

export function RouteList({
  apiDefintion,
  isLoading,
  limit,
  error,
}: {
  apiDefintion?: ApiDefinition;
  isLoading: boolean;
  limit?: number;
  error?: string | null;
}) {
  let routes = (apiDefintion?.routes || []) as ApiRoute[];
  routes = limit ? routes.slice(0, limit) : routes;
  // const [route, setRoute] = useState<ApiRoute | null>(null);
  const router=useRouter();
  const {apiId}=useCustomParam();
  return (
    <>
      <Box>
        {/* Active Deployments Section */}
        <Stack
          sx={{
            mb: 2,
          }}
        >
          <Typography variant="h6">Active Routes</Typography>

          {isLoading && <Loader className="self-center" />}
        </Stack>
        {!isLoading && !error && routes.length === 0 ? (
          <Typography variant="body2" className="text-muted-foreground">
            No routes defined for this API version.
          </Typography>
        ) : (
          //TODO: Add pagination List
          <>
            {error && <ErrorBoundary message={error} />}
            <List className="space-y-4 p-2">
              {apiDefintion &&
                routes?.map((route: ApiRoute) => {
                  const routeId = encodeURIComponent(`${route.path}|${route.method}`);
                  return (
                    <Box
                      key={`${apiDefintion.id}_${apiDefintion.version}_${route.method}_${route.path}`}
                      className="px-4 py-6 flex justify-between border rounded-lg dark:hover:bg-[#373737] hover:bg-[#C0C0C0] cursor-pointer"
                      onClick={(e) => {
                        e.preventDefault();
                        router.push(`/apis/${apiId}/${routeId}`)
                      }}
                    >
                      <Typography gutterBottom className="font-bold">
                        {route.path}
                      </Typography>
                      <Button2
                        variant="success"
                        size="xs"
                      >
                        {route.method}
                      </Button2>
                    </Box>
                  );
                })}
            </List>
          </>
        )}
      </Box>
    </>
  );
}

export default function RoutePage({
  apiId,
  version,
  limit,
}: {
  apiId: string;
  version?: string | null;
  limit?: number;
}) {
  //TODO to move this do separate custom hook so that we can resuse.
  const { isLoading, getApiDefintion, error: requestError } = useApiDefinitions(apiId, version);
  const { data: apiDefintion, error } = (!isLoading &&getApiDefintion()) || {};

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
