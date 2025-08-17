import { toast } from "@/hooks/use-toast";
import { invoke } from "@tauri-apps/api/core";
import { settingsService } from "@/lib/settings.ts";

export class CLIService {
  public callCLI = async (
    appId: string,
    command: string,
    subcommands: string[],
  ): Promise<unknown> => {
    // find folder location
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }
    //  we use the "invoke" here to call a special command that calls golem CLI for us
    let result: string;
    try {
      result = await invoke("call_golem_command", {
        command,
        subcommands,
        folderPath: app.folderLocation,
      });
    } catch (_e) {
      toast({
        title: "Error in calling golem CLI",
        description: String(_e),
        variant: "destructive",
        duration: 5000,
      });
      throw new Error("Error in calling golem CLI: " + String(_e));
    }

    let parsedResult;
    const match = result.match(/(\[.*]|\{.*})/s);
    if (match) {
      try {
        parsedResult = JSON.parse(match[0]);
      } catch {
        // some actions do not return JSON
      }
    }
    return parsedResult || true;
  };

  public callCLIWithLogs = async (
    appId: string,
    command: string,
    subcommands: string[],
  ): Promise<{ result: unknown; logs: string; success: boolean }> => {
    // find folder location
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }
    //  we use the "invoke" here to call a special command that calls golem CLI for us
    let result: string;
    let success = true;

    try {
      result = await invoke("call_golem_command", {
        command,
        subcommands,
        folderPath: app.folderLocation,
      });
    } catch (e) {
      success = false;
      result = String(e);
    }

    let parsedResult;
    const match = result.match(/(\[.*]|\{.*})/s);
    if (match) {
      try {
        parsedResult = JSON.parse(match[0]);
      } catch {
        // some actions do not return JSON
      }
    }

    return {
      result: parsedResult || true,
      logs: result,
      success,
    };
  };
}
