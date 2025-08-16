import { toast } from "@/hooks/use-toast";
import { Profile } from "@/types/index";
import { invoke } from "@tauri-apps/api/core";

export class ProfileService {
  /**
   * Get all available CLI profiles
   */
  public async getProfiles(): Promise<Profile[]> {
    return (await this.callCLIGlobal("profile", ["list"])) as Profile[];
  }

  /**
   * Get the currently active profile
   */
  public async getCurrentProfile(): Promise<Profile> {
    const profiles = await this.getProfiles();
    const activeProfile = profiles.find(p => p.is_active);
    if (!activeProfile) {
      throw new Error("No active profile found");
    }
    return activeProfile;
  }

  /**
   * Switch to a different profile
   */
  public async switchProfile(profileName: string): Promise<void> {
    await this.callCLIGlobal("profile", ["switch", profileName]);
  }

  /**
   * Get details for a specific profile
   */
  public async getProfileDetails(profileName?: string): Promise<Profile> {
    const args = ["get"];
    if (profileName) {
      args.push(profileName);
    }
    return (await this.callCLIGlobal("profile", args)) as Profile;
  }

  /**
   * Create a new profile
   */
  public async createProfile(
    profileKind: "Cloud" | "Oss",
    name: string,
    options: {
      setActive?: boolean;
      componentUrl?: string;
      workerUrl?: string;
      cloudUrl?: string;
      defaultFormat?: string;
    } = {},
  ): Promise<void> {
    const args = ["new", profileKind.toLowerCase(), name];

    if (options.setActive) {
      args.push("--set-active");
    }
    if (options.componentUrl) {
      args.push("--component-url", options.componentUrl);
    }
    if (options.workerUrl) {
      args.push("--worker-url", options.workerUrl);
    }
    if (options.cloudUrl) {
      args.push("--cloud-url", options.cloudUrl);
    }
    if (options.defaultFormat) {
      args.push("--default-format", options.defaultFormat);
    }

    await this.callCLIGlobal("profile", args);
  }

  /**
   * Delete a profile
   */
  public async deleteProfile(profileName: string): Promise<void> {
    await this.callCLIGlobal("profile", ["delete", profileName]);
  }

  /**
   * Global CLI call for profile commands (no app context needed)
   * Automatically adds --format=json and parses the response
   */
  private async callCLIGlobal(
    command: string,
    subcommands: string[],
  ): Promise<unknown> {
    let result: string;
    try {
      result = await invoke("call_golem_command", {
        command,
        subcommands,
        folderPath: "/", // Use root folder for global commands
      });
    } catch (_e) {
      toast({
        title: "Error in calling golem CLI",
        description: String(_e),
        variant: "destructive",
        duration: 5000,
      });
      throw new Error("Error in calling golem CLI");
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
  }
}

// Export a singleton instance
export const profileService = new ProfileService();
