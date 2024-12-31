/* eslint-disable @typescript-eslint/no-explicit-any */
import { Layers, PlusCircle } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button.tsx";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatRelativeTime } from "@/lib/utils";

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
      componentId: "17c50abc-d410-4603-a0d7-97d1a05cbad2",
      version: 0,
    },
  },
];

const ComponentsSection = () => {
  const navigate = useNavigate();
  const [components, setComponents] = useState(MockData);

  useEffect(() => {
    const fetchData = async () => {
      //https://release.api.golem.cloud/v1/components?project-id=305e832c-f7c1-4da6-babc-cb2422e0f5aa
      const response: any = await invoke("get_component");
      setComponents(response);
    };
    fetchData().then((r) => r);
  }, []);

  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6 overflow-scroll max-h-[50vh]">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-semibold">Components</h2>
        <button
          className="text-blue-600 hover:text-blue-700"
          onClick={() => {
            navigate("/components");
          }}
        >
          View All
        </button>
      </div>
      {components.length > 0 ? (
        <div className="p-4 pt-0 md:p-6 md:pt-0 flex-1 w-full">
          <div className="grid w-full grid-cols-1 gap-4 md:gap-6 md:grid-cols-2">
            {components.map((component) => (
              <div
                key={component.versionedComponentId.componentId}
                className="rounded-lg border bg-card text-card-foreground shadow transition-all hover:shadow-lg hover:shadow-border/75 duration-150 h-full flex-col gap-2 p-4"
                onClick={() =>
                  navigate(
                    `/components/${component.versionedComponentId.componentId}`
                  )
                }
              >
                <div className="flex h-12 flex-row items-start justify-between pb-2 text-base">
                  <div className="flex flex-col items-start">
                    <h3 className="font-medium">{component.componentName}</h3>
                    <span className="text-xs font-light text-muted-foreground">
                      {formatRelativeTime(component.createdAt)}
                    </span>
                  </div>
                  <div className="inline-flex items-center rounded-md px-2.5 py-0.5 text-xs transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 bg-primary-background text-primary-soft hover:bg-primary/50 active:bg-primary/50 border border-primary-border font-mono font-normal">
                    v{component.versionedComponentId.version}
                  </div>
                </div>
                <div className="mt-2 flex w-full items-center gap-2">
                  <div className="rounded-md border px-2.5 py-0.5 transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 hover:bg-accent hover:text-accent-foreground active:bg-accent/50 active:text-accent-foreground flex h-5 items-center gap-2 text-xs font-normal text-muted-foreground">
                    <span>
                      {component.metadata.exports[0].functions.length}
                    </span>
                    <span>Exports</span>
                  </div>
                  <div className="rounded-md border px-2.5 py-0.5 transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 hover:bg-accent hover:text-accent-foreground active:bg-accent/50 active:text-accent-foreground flex h-5 items-center gap-2 text-xs font-normal text-muted-foreground">
                    <span>{Math.round(component.componentSize / 1024)} KB</span>
                  </div>
                  <div className="rounded-md border px-2.5 py-0.5 transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 hover:bg-accent hover:text-accent-foreground active:bg-accent/50 active:text-accent-foreground flex h-5 items-center gap-2 text-xs font-normal text-muted-foreground">
                    <span>{component.componentType}</span>
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      ) : (
        <div className="flex flex-col items-center justify-center py-12 border-2 border-dashed border-gray-200 rounded-lg">
          <Layers className="h-12 w-12 text-gray-400 mb-4" />
          <h3 className="text-lg font-medium mb-2">No Components</h3>
          <p className="text-gray-500 mb-4">
            Create your first component to get started
          </p>
          <Button
            onClick={() => {
              navigate("/components/create");
            }}
          >
            <PlusCircle className="mr-2 size-4" />
            Create Component
          </Button>
        </div>
      )}
    </div>
  );
};

export default ComponentsSection;
