import {useNavigate, useParams, useSearchParams} from "react-router-dom";
import {useEffect, useState} from "react";
import {Edit, Trash2} from "lucide-react";

import {API} from "@/service";
import {Api, Route} from "@/types/api";
import ErrorBoundary from "@/components/errorBoundary.tsx";
import {Badge} from "@/components/ui/badge";
import {Button} from "@/components/ui/button";
import {Card, CardContent, CardHeader, CardTitle} from "@/components/ui/card";
import {Input} from "@/components/ui/input";
import {ComponentList} from "@/types/component";
import {HTTP_METHOD_COLOR} from "@/components/nav-route";

export const ApiRoute = () => {
    const navigate = useNavigate();
    const {apiName, version} = useParams();
    const [currentRoute, setCurrentRoute] = useState({} as Route);
    const [componentList, setComponentList] = useState<{
        [key: string]: ComponentList;
    }>({});
    const [queryParams] = useSearchParams();
    const path = queryParams.get("path");
    const method = queryParams.get("method");

    useEffect(() => {
        const fetchData = async () => {
            if (apiName && version && path && method) {
                const [apiResponse, componentResponse] = await Promise.all([
                    API.getApi(apiName),
                    API.getComponentByIdAsKey(),
                ]);
                setComponentList(componentResponse);
                const selectedApi = apiResponse.find((api) => api.version === version);
                if (selectedApi) {
                    const route = selectedApi.routes.find(
                        (route) => route.path === path && route.method === method
                    );
                    setCurrentRoute(route || ({} as Route));
                } else {
                    navigate(`/apis/${apiName}/version/${version}`);
                }
            } else {
                navigate(`/apis/${apiName}/version/${version}`);
            }
        };
        fetchData();
    }, [apiName, version, path, method]);

    const routeToQuery = () => {
        navigate(
            `/apis/${apiName}/version/${version}/routes/edit?path=${path}&method=${method}`
        );
    };

    const handleDelete = () => {
        if (apiName) {
            API.getApi(apiName).then((response: Api[]) => {
                const currentApi = response.find((api) => api.version === version);
                if (currentApi) {
                    currentApi.routes = currentApi.routes.filter(
                        (route) => route.path !== path! && route.method !== method!
                    );
                    API.putApi(apiName, version!, currentApi).then(() => {
                        navigate(`/apis/${apiName}/version/${version}`);
                    });
                }
            });
        }
    };

    return (
        <ErrorBoundary>
            <main className="flex-1 overflow-y-auto h-[80vh] mx-auto p-6 w-full max-w-7xl">
                <Card>
                    <CardHeader className="border-b border-zinc-800">
                        <div className="flex items-center justify-between">
                            <div className="flex items-center gap-2">
                                <Badge
                                    variant="secondary"
                                    className={
                                        HTTP_METHOD_COLOR[
                                            currentRoute.method as keyof typeof HTTP_METHOD_COLOR
                                            ]
                                    }
                                >
                                    {currentRoute.method}
                                </Badge>
                                <span className="text-sm font-mono">{currentRoute.path}</span>
                            </div>
                            <div className="flex gap-2">
                                <Button
                                    variant="secondary"
                                    size="sm"
                                    onClick={() => routeToQuery()}
                                >
                                    <Edit className="h-4 w-4 mr-1"/>
                                    Edit
                                </Button>
                                <Button
                                    variant="destructive"
                                    size="sm"
                                    onClick={() => {
                                        handleDelete();
                                    }}
                                >
                                    <Trash2 className="h-4 w-4 mr-1"/>
                                    Delete
                                </Button>
                            </div>
                        </div>
                    </CardHeader>
                    <CardContent className="space-y-6 pt-6">
                        <div className="space-y-2">
                            <CardTitle className="text-sm ">Component</CardTitle>
                            <Input
                                value={`${
                                    componentList[currentRoute?.binding?.componentId?.componentId]
                                        ?.componentName
                                } / v${currentRoute?.binding?.componentId?.version}`}
                                disabled
                                className="text-sm font-mono"
                            />
                        </div>

                        <div className="space-y-2">
                            <div className="flex items-center gap-2">
                                <CardTitle className="text-sm ">Path</CardTitle>
                                <Badge
                                    variant="outline"
                                    className="border-blue-500 text-blue-400"
                                >
                                    Parameters
                                </Badge>
                            </div>
                            <div className="grid grid-cols-2 gap-4">
                                <Input
                                    value="user-id"
                                    disabled
                                    className=" text-sm font-mono"
                                />
                            </div>
                        </div>

                        <div className="space-y-2">
                            <div className="flex items-center gap-2">
                                <CardTitle className="text-sm ">Response</CardTitle>
                                <Badge
                                    variant="outline"
                                    className="border-blue-500 text-blue-400"
                                >
                                    Rib
                                </Badge>
                            </div>
                            <div className="p-2 rounded-md border ">
                <pre
                    className="bg-gray-100 p-4 rounded-md text-sm font-mono dark:bg-gray-900 dark:text-gray-200 overflow-auto">
                  {currentRoute?.binding?.response || "No response"}
                </pre>
                            </div>
                        </div>

                        <div className="space-y-2">
                            <div className="flex items-center gap-2">
                                <CardTitle className="text-sm ">Worker Name</CardTitle>
                                <Badge
                                    variant="outline"
                                    className="border-blue-500 text-blue-400"
                                >
                                    Rib
                                </Badge>
                            </div>
                            <div className="p-2 rounded-md border ">
                <pre
                    className="bg-gray-100 p-4 rounded-md text-sm font-mono dark:bg-gray-900 dark:text-gray-200 overflow-auto">
                  {currentRoute?.binding?.workerName || "No worker name"}
                </pre>
                            </div>
                        </div>
                    </CardContent>
                </Card>
            </main>
        </ErrorBoundary>
    );
};
