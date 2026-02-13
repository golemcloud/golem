import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  type MockedFunction,
} from "vitest";
import { invoke } from "@tauri-apps/api/core";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@/hooks/use-toast", () => ({ toast: vi.fn() }));

import { profileService } from "../profile";

const mockedInvoke = invoke as MockedFunction<typeof invoke>;

describe("ProfileService CLI commands", () => {
  const service = profileService;

  beforeEach(() => {
    vi.clearAllMocks();
    mockedInvoke.mockResolvedValue("[]");
  });

  describe("getProfiles", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce(
        JSON.stringify([{ name: "local", is_active: true }]),
      );
      await service.getProfiles();
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "profile",
        subcommands: ["list"],
        folderPath: "/",
      });
    });
  });

  describe("switchProfile", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce("true");
      await service.switchProfile("my-profile");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "profile",
        subcommands: ["switch", "my-profile"],
        folderPath: "/",
      });
    });
  });

  describe("getProfileDetails", () => {
    it("sends correct command without name", async () => {
      mockedInvoke.mockResolvedValueOnce(JSON.stringify({ name: "local" }));
      await service.getProfileDetails();
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "profile",
        subcommands: ["get"],
        folderPath: "/",
      });
    });

    it("sends correct command with name", async () => {
      mockedInvoke.mockResolvedValueOnce(
        JSON.stringify({ name: "my-profile" }),
      );
      await service.getProfileDetails("my-profile");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "profile",
        subcommands: ["get", "my-profile"],
        folderPath: "/",
      });
    });
  });

  describe("createProfile", () => {
    it("sends correct command for Cloud profile (minimal)", async () => {
      mockedInvoke.mockResolvedValueOnce("true");
      await service.createProfile("Cloud", "prof1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "profile",
        subcommands: ["new", "cloud", "prof1"],
        folderPath: "/",
      });
    });

    it("sends correct command with all options", async () => {
      mockedInvoke.mockResolvedValueOnce("true");
      await service.createProfile("Cloud", "prof1", {
        setActive: true,
        componentUrl: "http://comp.url",
        agentUrl: "http://agent.url",
        cloudUrl: "http://cloud.url",
        defaultFormat: "json",
      });
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "profile",
        subcommands: [
          "new",
          "cloud",
          "prof1",
          "--set-active",
          "--component-url",
          "http://comp.url",
          "--agent-url",
          "http://agent.url",
          "--cloud-url",
          "http://cloud.url",
          "--default-format",
          "json",
        ],
        folderPath: "/",
      });
    });

    it("sends correct command for Oss with partial options", async () => {
      mockedInvoke.mockResolvedValueOnce("true");
      await service.createProfile("Oss", "oss-prof", {
        componentUrl: "http://comp.url",
      });
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "profile",
        subcommands: [
          "new",
          "oss",
          "oss-prof",
          "--component-url",
          "http://comp.url",
        ],
        folderPath: "/",
      });
    });
  });

  describe("deleteProfile", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce("true");
      await service.deleteProfile("my-profile");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "profile",
        subcommands: ["delete", "my-profile"],
        folderPath: "/",
      });
    });
  });
});
