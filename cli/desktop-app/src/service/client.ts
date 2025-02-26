/* eslint-disable @typescript-eslint/no-explicit-any */
import { toast } from "@/hooks/use-toast";
import { fetchData, updateIP } from "@/lib/tauri&web.ts";
import { ENDPOINT } from "@/service/endpoints.ts";
import { parseErrorResponse } from "@/service/error-handler.ts";
import { Api } from "@/types/api.ts";
import { Component, ComponentList } from "@/types/component.ts";
import { Plugin } from "@/types/plugin";

export class Service {
  public baseUrl: string;

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl;
  }

  public updateBackendEndpoint = async (url: string) => {
    await updateIP(url);
    this.baseUrl = url;
  };

  /**
   * getComponents: Get the list of all components
   * Note: Sample Endpoint https://release.api.golem.cloud/v1/components
   * @returns {Promise<Component[]>}
   */
  public checkHealth = async () => {
    const r = await this.callApi("/healthcheck");
    return r;
  };

  /**
   * getComponents: Get the list of all components
   * Note: Sample Endpoint https://release.api.golem.cloud/v1/components
   * @returns {Promise<Component[]>}
   */
  public getComponents = async (): Promise<Component[]> => {
    const r = await this.callApi(ENDPOINT.getComponents());
    return r as Component[];
  };

  public getComponentById = async (id: string) => {
    const r = await this.callApi(ENDPOINT.getComponentById(id));
    return r as Component[];
  };

  public getComponentByIdAndVersion = async (id: string, version: number) => {
    const r = await this.callApi(
      ENDPOINT.getComponentByIdAndVersion(id, version),
    );
    return r as Component;
  };

  public createComponent = async (form: FormData) => {
    try {
      const response = await fetchData(`${this.baseUrl}/v1/components`, {
        method: "POST",
        body: form,
      });

      if (!response.ok) {
        // Handle HTTP errors (e.g., 400, 500)
        const errorText = await response.text(); // Try to get error details
        throw new Error(`HTTP Error ${response.status}: ${errorText}`);
      }

      return await response.json(); // Return JSON response if successful
    } catch (error) {
      console.error("Error in createComponent:", error);
      parseErrorResponse(error);
    }
  };

  public getComponentByName = async (name: string) => {
    const r = await this.callApi(
      `${ENDPOINT.getComponents()}?component-name=${name}`,
    );
    return r as Component;
  };

  public updateComponent = async (componenetId: string, form: FormData) => {
    try {
      const response = await fetchData(
        `${this.baseUrl}/v1/components/${componenetId}/updates`,
        {
          method: "POST",
          body: form,
        },
      );

      if (!response.ok) {
        // Handle HTTP errors (e.g., 400, 500)
        const errorText = await response.text(); // Try to get error details
        throw new Error(`HTTP Error ${response.status}: ${errorText}`);
      }

      return await response.json(); // Return JSON response if successful
    } catch (error) {
      console.error("Error in Update Component:", error);
    }
  };

  public deletePluginToComponent = async (
    id: string,
    installation_id: string,
  ) => {
    return await this.callApi(
      ENDPOINT.deletePluginToComponent(id, installation_id),
      "DELETE",
    );
  };

  public addPluginToComponent = async (id: string, form: any) => {
    return await this.callApi(
      ENDPOINT.addPluginToComponent(id),
      "POST",
      JSON.stringify(form),
    );
  };

  public upgradeWorker = async (
    componentId: string,
    workerName: string,
    version: number,
    upgradeType: string,
  ) => {
    return await this.callApi(
      ENDPOINT.updateWorker(componentId, workerName),
      "POST",
      JSON.stringify({
        mode: upgradeType,
        targetVersion: version,
      }),
      {
        "Content-Type": "application/json; charset=utf-8",
      },
    );
  };

  public findWorker = async (
    componentId: string,
    param = { count: 100, precise: true },
  ) => {
    const r = await this.callApi(
      ENDPOINT.findWorker(componentId),
      "POST",
      JSON.stringify(param),
    );
    return r;
  };

  public deleteWorker = async (componentId: string, workName: string) => {
    const r = await this.callApi(
      ENDPOINT.deleteWorker(componentId, workName),
      "DELETE",
    );
    return r;
  };

  public createWorker = async (componentID: string, params: any) => {
    const r = await this.callApi(
      ENDPOINT.createWorker(componentID),
      "POST",
      JSON.stringify(params),
    );
    return r;
  };

  public getApiList = async (): Promise<Api[]> => {
    const r = await this.callApi(ENDPOINT.getApiList());
    return r as Api[];
  };

  public getApi = async (id: string): Promise<Api[]> => {
    const r = await this.callApi(ENDPOINT.getApi(id));
    return r as Api[];
  };

  public createApi = async (payload: Api) => {
    const r = await this.callApi(
      ENDPOINT.createApi(),
      "POST",
      JSON.stringify(payload),
    );
    return r;
  };

  public deleteApi = async (id: string, version: string) => {
    const r = await this.callApi(ENDPOINT.deleteApi(id, version), "DELETE");
    return r;
  };

  public putApi = async (id: string, version: string, payload: Api) => {
    const r = await this.callApi(
      ENDPOINT.putApi(id, version),
      "PUT",
      JSON.stringify(payload),
    );
    return r;
  };

  public postApi = async (payload: Api) => {
    const r = await this.callApi(
      ENDPOINT.postApi(),
      "POST",
      JSON.stringify(payload),
    );
    return r;
  };

  public getParticularWorker = async (
    componentId: string,
    workerName: string,
  ) => {
    const r = await this.callApi(
      ENDPOINT.getParticularWorker(componentId, workerName),
    );
    return r;
  };

  public interruptWorker = async (componentId: string, workerName: string) => {
    const r = await this.callApi(
      ENDPOINT.interruptWorker(componentId, workerName),
      "POST",
      JSON.stringify({}),
    );
    return r;
  };

  public resumeWorker = async (componentId: string, workerName: string) => {
    const r = await this.callApi(
      ENDPOINT.resumeWorker(componentId, workerName),
      "POST",
      JSON.stringify({}),
    );
    return r;
  };

  public invokeWorkerAwait = async (
    componentId: string,
    workerName: string,
    functionName: string,
    payload: any,
  ) => {
    const r = await this.callApi(
      ENDPOINT.invokeWorker(componentId, workerName, functionName),
      "POST",
      JSON.stringify(payload),
    );
    return r;
  };

  public invokeEphemeralAwait = async (
    componentId: string,
    functionName: string,
    payload: any,
  ) => {
    const r = await this.callApi(
      ENDPOINT.invokeEphemeralWorker(componentId, functionName),
      "POST",
      JSON.stringify(payload),
    );
    return r;
  };

  public getDeploymentApi = async (versionId: string) => {
    const r = await this.callApi(ENDPOINT.getDeploymentApi(versionId));
    return r;
  };

  public deleteDeployment = async (deploymentId: string) => {
    const r = await this.callApi(
      ENDPOINT.deleteDeployment(deploymentId),
      "DELETE",
    );
    return r;
  };

  public createDeployment = async (payload: any) => {
    const r = await this.callApi(
      ENDPOINT.createDeployment(),
      "POST",
      JSON.stringify(payload),
    );
    return r;
  };

  public getOplog = async (
    componentId: string,
    workerName: string,
    count: number,
    searchQuery: string,
  ) => {
    const r = await this.callApi(
      ENDPOINT.getOplog(componentId, workerName, count, searchQuery),
      "GET",
    );
    return r;
  };

  public getComponentByIdAsKey = async (): Promise<
    Record<string, ComponentList>
  > => {
    // Assume getComponents returns a Promise<RawComponent[]>
    const components = await this.getComponents();

    const componentList = components.reduce<Record<string, ComponentList>>(
      (acc, component) => {
        const {
          componentName,
          versionedComponentId = {},
          componentType,
        } = component;

        // Use type assertion to help TS with the optional fields in versionedComponentId
        const { componentId = "", version = 0 } = versionedComponentId;

        // Use componentId as the key. If not available, you might want to skip or handle differently.
        const key = componentId || "";

        // Initialize the component entry if it doesn't exist
        if (!acc[key]) {
          acc[key] = {
            componentName: componentName || "",
            componentId: componentId || "",
            componentType: componentType || "",
            versions: [],
            versionList: [],
          };
        }
        if (acc[key].versionList) {
          acc[key].versionList.push(version);
        }
        if (acc[key].versions) {
          acc[key].versions.push(component);
        }
        return acc;
      },
      {},
    );
    return componentList;
  };

  public getPlugins = async (): Promise<Plugin[]> => {
    return await this.callApi(ENDPOINT.getPlugins());
  };

  public getPluginByName = async (name: string): Promise<Plugin[]> => {
    return await this.callApi(ENDPOINT.getPluginName(name));
  };

  public downloadComponent = async (
    componentId: string,
    version: number,
  ): Promise<any> => {
    return await this.downloadApi(
      ENDPOINT.downloadComponent(componentId, version),
    );
  };
  public createPlugin = async (payload: Plugin) => {
    return await this.callApi(
      ENDPOINT.getPlugins(),
      "POST",
      JSON.stringify(payload),
    );
  };
  public deletePlugin = async (name: string, version: string) => {
    return await this.callApi(ENDPOINT.deletePlugin(name, version), "DELETE");
  };

  private callApi = async (
    url: string,
    method: string = "GET",
    data: FormData | string | null = null,
    headers = { "Content-Type": "application/json" },
  ): Promise<any> => {
    try {
      const response = await fetchData(`${this.baseUrl}${url}`, {
        method: method,
        body: data,
        headers: headers,
      });

      const contentType = response.headers.get("Content-Type");
      let responseData: any;

      if (contentType && contentType.includes("application/json")) {
        responseData = await response.json();
      } else {
        responseData = await response.text();
      }

      if (response.ok) {
        return responseData;
      } else {
        if (response.status === 504) {
          return;
        }

        throw responseData;
      }
    } catch (response: any) {
      const result = parseErrorResponse(response);
      if (response?.status !== 504) {
        throw result;
      }
    }
  };

  private downloadApi = async (
    url: string,
    method: string = "GET",
    data: FormData | string | null = null,
    headers = { "Content-Type": "application/json" },
  ): Promise<any> => {
    const resp = await fetchData(`${this.baseUrl}${url}`, {
      method: method,
      body: data,
      headers: headers,
    })
      .then(res => {
        if (res.ok) {
          return res;
        }
      })
      .catch(err => {
        toast({
          title: "Api is Failed check the api details",
          variant: "destructive",
          duration: 5000,
        });
        throw err;
      });
    return resp;
  };
}
