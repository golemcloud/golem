/* eslint-disable @typescript-eslint/no-explicit-any */
/* eslint-disable @typescript-eslint/no-unused-vars */
import { useState, useEffect } from "react";
import { Search, LayoutGrid, PlusCircle } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";

import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button.tsx";
import { formatRelativeTime } from "@/lib/utils";
import { Input } from "@/components/ui/input";

const MockData = [
  {
    componentName: "Component",
    componentSize: 129179,
    componentType: "Durable",
    createdAt: "2024-12-31T09:43:54.427307+00:00",
    files: [],
    installedPlugins: [],
    metadata: {
      exports: [
        {
          functions: [
            {
              name: "initialize-cart",
              parameters: [
                {
                  name: "user-id",
                  typ: {
                    type: "Str",
                  },
                },
              ],
              results: [],
            },
            {
              name: "add-item",
              parameters: [
                {
                  name: "item",
                  typ: {
                    fields: [
                      {
                        name: "product-id",
                        typ: {
                          type: "Str",
                        },
                      },
                      {
                        name: "name",
                        typ: {
                          type: "Str",
                        },
                      },
                      {
                        name: "price",
                        typ: {
                          type: "F32",
                        },
                      },
                      {
                        name: "quantity",
                        typ: {
                          type: "U32",
                        },
                      },
                    ],
                    type: "Record",
                  },
                },
              ],
              results: [],
            },
            {
              name: "remove-item",
              parameters: [
                {
                  name: "product-id",
                  typ: {
                    type: "Str",
                  },
                },
              ],
              results: [],
            },
            {
              name: "update-item-quantity",
              parameters: [
                {
                  name: "product-id",
                  typ: {
                    type: "Str",
                  },
                },
                {
                  name: "quantity",
                  typ: {
                    type: "U32",
                  },
                },
              ],
              results: [],
            },
            {
              name: "checkout",
              parameters: [],
              results: [
                {
                  name: null,
                  typ: {
                    cases: [
                      {
                        name: "error",
                        typ: {
                          type: "Str",
                        },
                      },
                      {
                        name: "success",
                        typ: {
                          fields: [
                            {
                              name: "order-id",
                              typ: {
                                type: "Str",
                              },
                            },
                          ],
                          type: "Record",
                        },
                      },
                    ],
                    type: "Variant",
                  },
                },
              ],
            },
            {
              name: "get-cart-contents",
              parameters: [],
              results: [
                {
                  name: null,
                  typ: {
                    inner: {
                      fields: [
                        {
                          name: "product-id",
                          typ: {
                            type: "Str",
                          },
                        },
                        {
                          name: "name",
                          typ: {
                            type: "Str",
                          },
                        },
                        {
                          name: "price",
                          typ: {
                            type: "F32",
                          },
                        },
                        {
                          name: "quantity",
                          typ: {
                            type: "U32",
                          },
                        },
                      ],
                      type: "Record",
                    },
                    type: "List",
                  },
                },
              ],
            },
          ],
          name: "golem:component/api",
          type: "Instance",
        },
      ],
      memories: [
        {
          initial: 1114112,
          maximum: null,
        },
      ],
      producers: [
        {
          fields: [
            {
              name: "processed-by",
              values: [
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
                {
                  name: "cargo-component",
                  version: "0.13.2 (wasi:040ec92)",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "language",
              values: [
                {
                  name: "Rust",
                  version: "",
                },
                {
                  name: "C11",
                  version: "",
                },
              ],
            },
            {
              name: "processed-by",
              values: [
                {
                  name: "rustc",
                  version: "1.83.0 (90b35a623 2024-11-26)",
                },
                {
                  name: "clang",
                  version:
                    "18.1.2-wasi-sdk (https://github.com/llvm/llvm-project 26a1d6601d727a96f4301d0d8647b5a42760ae0c)",
                },
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
                {
                  name: "wit-bindgen-rust",
                  version: "0.25.0",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "language",
              values: [
                {
                  name: "Rust",
                  version: "",
                },
              ],
            },
            {
              name: "processed-by",
              values: [
                {
                  name: "rustc",
                  version: "1.75.0 (82e1608df 2023-12-21)",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "processed-by",
              values: [
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "processed-by",
              values: [
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
              ],
            },
          ],
        },
      ],
    },
    projectId: "305e832c-f7c1-4da6-babc-cb2422e0f5aa",
    versionedComponentId: {
      componentId: "17c50abc-d410-4603-a0d7-ere",
      version: 0,
    },
  },
  {
    componentName: "Component",
    componentSize: 129179,
    componentType: "Durable",
    createdAt: "2025-01-01T11:33:25.892716+00:00",
    files: [],
    installedPlugins: [],
    metadata: {
      exports: [
        {
          functions: [
            {
              name: "initialize-cart",
              parameters: [
                {
                  name: "user-id",
                  typ: {
                    type: "Str",
                  },
                },
              ],
              results: [],
            },
            {
              name: "add-item",
              parameters: [
                {
                  name: "item",
                  typ: {
                    fields: [
                      {
                        name: "product-id",
                        typ: {
                          type: "Str",
                        },
                      },
                      {
                        name: "name",
                        typ: {
                          type: "Str",
                        },
                      },
                      {
                        name: "price",
                        typ: {
                          type: "F32",
                        },
                      },
                      {
                        name: "quantity",
                        typ: {
                          type: "U32",
                        },
                      },
                    ],
                    type: "Record",
                  },
                },
              ],
              results: [],
            },
            {
              name: "remove-item",
              parameters: [
                {
                  name: "product-id",
                  typ: {
                    type: "Str",
                  },
                },
              ],
              results: [],
            },
            {
              name: "update-item-quantity",
              parameters: [
                {
                  name: "product-id",
                  typ: {
                    type: "Str",
                  },
                },
                {
                  name: "quantity",
                  typ: {
                    type: "U32",
                  },
                },
              ],
              results: [],
            },
            {
              name: "checkout",
              parameters: [],
              results: [
                {
                  name: null,
                  typ: {
                    cases: [
                      {
                        name: "error",
                        typ: {
                          type: "Str",
                        },
                      },
                      {
                        name: "success",
                        typ: {
                          fields: [
                            {
                              name: "order-id",
                              typ: {
                                type: "Str",
                              },
                            },
                          ],
                          type: "Record",
                        },
                      },
                    ],
                    type: "Variant",
                  },
                },
              ],
            },
            {
              name: "get-cart-contents",
              parameters: [],
              results: [
                {
                  name: null,
                  typ: {
                    inner: {
                      fields: [
                        {
                          name: "product-id",
                          typ: {
                            type: "Str",
                          },
                        },
                        {
                          name: "name",
                          typ: {
                            type: "Str",
                          },
                        },
                        {
                          name: "price",
                          typ: {
                            type: "F32",
                          },
                        },
                        {
                          name: "quantity",
                          typ: {
                            type: "U32",
                          },
                        },
                      ],
                      type: "Record",
                    },
                    type: "List",
                  },
                },
              ],
            },
          ],
          name: "golem:component/api",
          type: "Instance",
        },
      ],
      memories: [
        {
          initial: 1114112,
          maximum: null,
        },
      ],
      producers: [
        {
          fields: [
            {
              name: "processed-by",
              values: [
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
                {
                  name: "cargo-component",
                  version: "0.13.2 (wasi:040ec92)",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "language",
              values: [
                {
                  name: "Rust",
                  version: "",
                },
                {
                  name: "C11",
                  version: "",
                },
              ],
            },
            {
              name: "processed-by",
              values: [
                {
                  name: "rustc",
                  version: "1.83.0 (90b35a623 2024-11-26)",
                },
                {
                  name: "clang",
                  version:
                    "18.1.2-wasi-sdk (https://github.com/llvm/llvm-project 26a1d6601d727a96f4301d0d8647b5a42760ae0c)",
                },
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
                {
                  name: "wit-bindgen-rust",
                  version: "0.25.0",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "language",
              values: [
                {
                  name: "Rust",
                  version: "",
                },
              ],
            },
            {
              name: "processed-by",
              values: [
                {
                  name: "rustc",
                  version: "1.75.0 (82e1608df 2023-12-21)",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "processed-by",
              values: [
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "processed-by",
              values: [
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
              ],
            },
          ],
        },
      ],
    },
    projectId: "305e832c-f7c1-4da6-babc-cb2422e0f5aa",
    versionedComponentId: {
      componentId: "17c50abc-d410-4603-a0d7-ererer",
      version: 1,
    },
  },
  {
    componentName: "componentNew",
    componentSize: 129179,
    componentType: "Durable",
    createdAt: "2025-01-01T13:10:17.260278+00:00",
    files: [],
    installedPlugins: [],
    metadata: {
      exports: [
        {
          functions: [
            {
              name: "initialize-cart",
              parameters: [
                {
                  name: "user-id",
                  typ: {
                    type: "Str",
                  },
                },
              ],
              results: [],
            },
            {
              name: "add-item",
              parameters: [
                {
                  name: "item",
                  typ: {
                    fields: [
                      {
                        name: "product-id",
                        typ: {
                          type: "Str",
                        },
                      },
                      {
                        name: "name",
                        typ: {
                          type: "Str",
                        },
                      },
                      {
                        name: "price",
                        typ: {
                          type: "F32",
                        },
                      },
                      {
                        name: "quantity",
                        typ: {
                          type: "U32",
                        },
                      },
                    ],
                    type: "Record",
                  },
                },
              ],
              results: [],
            },
            {
              name: "remove-item",
              parameters: [
                {
                  name: "product-id",
                  typ: {
                    type: "Str",
                  },
                },
              ],
              results: [],
            },
            {
              name: "update-item-quantity",
              parameters: [
                {
                  name: "product-id",
                  typ: {
                    type: "Str",
                  },
                },
                {
                  name: "quantity",
                  typ: {
                    type: "U32",
                  },
                },
              ],
              results: [],
            },
            {
              name: "checkout",
              parameters: [],
              results: [
                {
                  name: null,
                  typ: {
                    cases: [
                      {
                        name: "error",
                        typ: {
                          type: "Str",
                        },
                      },
                      {
                        name: "success",
                        typ: {
                          fields: [
                            {
                              name: "order-id",
                              typ: {
                                type: "Str",
                              },
                            },
                          ],
                          type: "Record",
                        },
                      },
                    ],
                    type: "Variant",
                  },
                },
              ],
            },
            {
              name: "get-cart-contents",
              parameters: [],
              results: [
                {
                  name: null,
                  typ: {
                    inner: {
                      fields: [
                        {
                          name: "product-id",
                          typ: {
                            type: "Str",
                          },
                        },
                        {
                          name: "name",
                          typ: {
                            type: "Str",
                          },
                        },
                        {
                          name: "price",
                          typ: {
                            type: "F32",
                          },
                        },
                        {
                          name: "quantity",
                          typ: {
                            type: "U32",
                          },
                        },
                      ],
                      type: "Record",
                    },
                    type: "List",
                  },
                },
              ],
            },
          ],
          name: "golem:component/api",
          type: "Instance",
        },
      ],
      memories: [
        {
          initial: 1114112,
          maximum: null,
        },
      ],
      producers: [
        {
          fields: [
            {
              name: "processed-by",
              values: [
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
                {
                  name: "cargo-component",
                  version: "0.13.2 (wasi:040ec92)",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "language",
              values: [
                {
                  name: "Rust",
                  version: "",
                },
                {
                  name: "C11",
                  version: "",
                },
              ],
            },
            {
              name: "processed-by",
              values: [
                {
                  name: "rustc",
                  version: "1.83.0 (90b35a623 2024-11-26)",
                },
                {
                  name: "clang",
                  version:
                    "18.1.2-wasi-sdk (https://github.com/llvm/llvm-project 26a1d6601d727a96f4301d0d8647b5a42760ae0c)",
                },
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
                {
                  name: "wit-bindgen-rust",
                  version: "0.25.0",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "language",
              values: [
                {
                  name: "Rust",
                  version: "",
                },
              ],
            },
            {
              name: "processed-by",
              values: [
                {
                  name: "rustc",
                  version: "1.75.0 (82e1608df 2023-12-21)",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "processed-by",
              values: [
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
              ],
            },
          ],
        },
        {
          fields: [
            {
              name: "processed-by",
              values: [
                {
                  name: "wit-component",
                  version: "0.208.1",
                },
              ],
            },
          ],
        },
      ],
    },
    projectId: "305e832c-f7c1-4da6-babc-cb2422e0f5aa",
    versionedComponentId: {
      componentId: "1b5260e8-a8ae-4ca7-bced-179586082da5",
      version: 0,
    },
  },
];

const WorkersMockData = {
  cursor: null,
  workers: [
    {
      accountId: "5f60f26f-da99-40e7-90fe-33cc93f55a1f",
      activePlugins: [],
      args: [],
      componentSize: 129179,
      componentVersion: 0,
      createdAt: "2025-01-01T13:18:05.024Z",
      env: {},
      lastError: null,
      ownedResources: {},
      pendingInvocationCount: 0,
      retryCount: 0,
      status: "Idle",
      totalLinearMemorySize: 1114112,
      updates: [],
      workerId: {
        componentId: "1b5260e8-a8ae-4ca7-bced-179586082da5",
        workerName: "51384dde-4d3e-4088-9e9c-c66b7d5e02d5",
      },
    },
    {
      accountId: "5f60f26f-da99-40e7-90fe-33cc93f55a1f",
      activePlugins: [],
      args: [],
      componentSize: 129179,
      componentVersion: 0,
      createdAt: "2025-01-01T13:12:09.317Z",
      env: {
        EMAILID: "EMAIL",
      },
      lastError: null,
      ownedResources: {},
      pendingInvocationCount: 0,
      retryCount: 0,
      status: "Idle",
      totalLinearMemorySize: 1114112,
      updates: [],
      workerId: {
        componentId: "1b5260e8-a8ae-4ca7-bced-179586082da5",
        workerName: "list-1",
      },
    },
  ],
};

const Metrix = ["Idle", "Running", "Suspended", "Failed"];

const Components = () => {
  const navigate = useNavigate();
  const [componentList, setComponentList] = useState({});
  const [componentApiList, setComponentApiList] = useState({});
  const [workerList, setWorkerList] = useState({} as any);

  useEffect(() => {
    const fetchData = async () => {
      //https://release.api.golem.cloud/v1/components?project-id=305e832c-f7c1-4da6-babc-cb2422e0f5aa
      // const response: any = await invoke("get_component");
      const componentData = {} as any;
      const response: any = MockData;
      response.forEach((data: any) => {
        componentData[data.versionedComponentId.componentId] = {
          componentName: data.componentName,
          componentId: data.versionedComponentId.componentId,
          createdAt: data.createdAt,
          exports: data.metadata.exports,
          componentSize: data.componentSize,
          componentType: data.componentType,
          versionId: [
            ...(componentData[data.versionedComponentId.componentId]
              ?.versionId || []),
            data.versionedComponentId.version,
          ],
        };
      });
      setComponentApiList(componentData);
      setComponentList(componentData);
    };
    fetchData().then((r) => r);

    // fetching to get Workers details

    const fetchData2 = async () => {
      //https://release.api.golem.cloud/v1/components/1b5260e8-a8ae-4ca7-bced-179586082da5/workers/find
      //get method
      // const response: any = await invoke("get_component");
      const response: any = WorkersMockData;
      const workerData = {} as any;
      response.workers.forEach((data: any) => {
        const exisitngData = workerData[data.workerId.componentId] || {};
        switch (data.status) {
          case "Idle":
            if (exisitngData.Idle) {
              exisitngData.Idle++;
            } else {
              exisitngData["Idle"] = 1;
            }
            break;
          case "Running":
            if (exisitngData.Running) {
              exisitngData.Running++;
            } else {
              exisitngData["Running"] = 1;
            }
            break;
          case "Suspended":
            if (exisitngData.Suspended) {
              exisitngData.Suspended++;
            } else {
              exisitngData["Suspended"] = 1;
            }
            break;
          case "Failed":
            if (exisitngData.Failed) {
              exisitngData.Failed++;
            } else {
              exisitngData["Failed"] = 1;
            }
            break;
          default:
        }
        workerData[data.workerId.componentId] = exisitngData;
      });
      setWorkerList(workerData);
    };
    fetchData2().then((r) => r);
  }, []);

  const handleSearch = (e: any) => {
    const value = e.target.value;
    const filteredList = Object.fromEntries(
      Object.entries(componentApiList).filter(([_, data]: [string, any]) =>
        data.componentName.toLowerCase().includes(value)
      )
    );

    setComponentList(filteredList);
  };

  return (
    <div className="container mx-auto px-4 py-8">
      <div className="flex flex-wrap items-center justify-between gap-4 mb-8">
        <div className="relative flex-1">
          <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-gray-400 h-5 w-5" />
          <Input
            type="text"
            placeholder="Search Components..."
            className="w-full pl-10 pr-4 py-2"
            onChange={(e) => handleSearch(e)}
          />
        </div>
        <div className="flex items-center gap-2">
          <Button onClick={() => navigate("/components/create")}>
            <PlusCircle className="mr-2 size-4" />
            Create Component
          </Button>
        </div>
      </div>

      {Object.keys(componentList).length === 0 ? (
        <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
          <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
            <LayoutGrid className="h-8 w-8 text-gray-400" />
          </div>
          <h2 className="text-xl font-semibold mb-2 text-center">
            No Project Components
          </h2>
          <p className="text-gray-500 mb-6 text-center">
            Create a new component to get started.
          </p>
        </div>
      ) : (
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 overflow-scroll max-h-[78vh]">
          {Object.values(componentList).map((data: any) => (
            <Card
              key={data.componentId}
              className="border shadow-sm cursor-pointer"
              onClick={() => navigate(`/components/${data.componentId}`)}
            >
              <CardHeader className="pb-4">
                <CardTitle className="text-lg font-medium">
                  {data.componentName}
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="grid grid-cols-2 sm:grid-cols-4 :grid-cols-4  gap-2">
                  {Metrix.map((metric) => (
                    <div
                      key={metric}
                      className="flex flex-col items-start space-y-1"
                    >
                      <span className="text-sm text-muted-foreground">
                        {metric}
                      </span>
                      <span className="text-lg font-medium">
                        {workerList[data.componentId]?.[metric] || 0}
                      </span>
                    </div>
                  ))}
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <Badge variant="secondary" className="rounded-md">
                    V{data.versionId?.[0]}
                  </Badge>
                  <Badge variant="secondary" className="rounded-md">
                    {data.exports[0]?.functions.length} Exports
                  </Badge>
                  <Badge variant="secondary" className="rounded-md">
                    {Math.round(data.componentSize / 1024)} KB
                  </Badge>
                  <Badge variant="secondary" className="rounded-md">
                    {data.componentType}
                  </Badge>
                  <span className="ml-auto text-sm text-muted-foreground">
                    {formatRelativeTime(data.createdAt)}
                  </span>
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      )}
    </div>
  );
};

export default Components;
