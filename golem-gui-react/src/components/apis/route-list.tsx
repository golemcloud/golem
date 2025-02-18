"use client";
import { Box, Typography, Stack, List } from "@mui/material";
import { Loader } from "lucide-react";
import { ApiDefinition, ApiRoute } from "@lib/types/api";
import ErrorBoundary from "@ui/error-boundary";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useCustomParam } from "@lib/hooks/use-custom-param";
import { Button2 } from "@components/ui/button";

interface RouteProps {
    apiDefintion?: ApiDefinition;
    isLoading: boolean;
    limit?: number;
    error?: string | null;
  }
  
  export function RouteList({
    apiDefintion,
    isLoading,
    limit,
    error,
  }: RouteProps) {
    let routes = (apiDefintion?.routes || []) as ApiRoute[];
    routes = limit ? routes.slice(0, limit) : routes;
    const navigate=useNavigate();
    const {apiId}=useCustomParam();
    const [params] = useSearchParams();
    const version= params?.get("version");
    return (
      <>
        <Box>
          <Stack
            className="mb-4"
          >
            <Typography variant="h6">Active Routes</Typography>
  
            {isLoading && <Loader className="self-center" />}
          </Stack>
          {!isLoading && !error && routes.length === 0 ? (
            <Typography variant="body2" className="text-muted-foreground">
              No routes defined for this API version.
            </Typography>
          ) : (
            <>
              {error && <ErrorBoundary message={error} />}
              <List className="space-y-4 p-2">
                {apiDefintion &&
                  routes?.map((route: ApiRoute) => {
                    const routeId = encodeURIComponent(`${route.path}|${route.method}`);
                    return (
                      <Box
                        key={`${apiDefintion.id}_${apiDefintion.version}_${route.method}_${route.path}`}
                        className="px-4 py-6 flex justify-between border rounded-lg hover:bg-silver cursor-pointer"
                        onClick={(e) => {
                          e.preventDefault();
                          navigate(`/apis/${apiId}/${routeId}${version? `?version=${version}`: ''}`)
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
  