import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ConnectionTestResult,
  PlatformCapabilities,
  ProviderCredentialStatus,
} from "../ipc/tauri";
import { OnboardingPanel } from "./OnboardingPanel";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const defaultStatus: ProviderCredentialStatus = {
  configured: false,
  masked_preview: null,
  secret_service_available: true,
  degraded_reason: null,
  guidance: null,
};

const defaultCapabilities: PlatformCapabilities = {
  display_server: "linux_x11",
  microphone_permission: "not_applicable",
  accessibility_permission: "not_applicable",
  supports_global_hotkey: true,
  supports_text_insertion: true,
  wayland_helpers: [],
  secret_service_available: true,
};

describe("OnboardingPanel at Tauri IPC boundary", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (command: string) => {
      switch (command) {
        case "get_provider_credential_status":
          return defaultStatus;
        case "get_platform_capabilities":
          return defaultCapabilities;
        default:
          return null;
      }
    });
  });

  it("rejects empty Provider Credential input and saves/deletes valid keys via native keyring IPC", async () => {
    render(<OnboardingPanel />);

    await waitFor(() => {
      expect(
        screen.getByLabelText(/provider credential/i),
      ).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: /save credential/i }));
    expect(
      screen.getByText(/provider credential cannot be empty/i),
    ).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "save_provider_credential",
      expect.anything(),
    );

    invokeMock.mockImplementation(async (command: string) => {
      if (command === "save_provider_credential") {
        return {
          ...defaultStatus,
          configured: true,
          masked_preview: "••••1234",
        } satisfies ProviderCredentialStatus;
      }
      if (command === "delete_provider_credential") {
        return defaultStatus;
      }
      if (command === "get_provider_credential_status") {
        return defaultStatus;
      }
      if (command === "get_platform_capabilities") {
        return defaultCapabilities;
      }
      return null;
    });

    fireEvent.change(screen.getByLabelText(/provider credential/i), {
      target: { value: "AIzaSyValidKey1234" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save credential/i }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("save_provider_credential", {
        apiKey: "AIzaSyValidKey1234",
      });
      expect(screen.getByText(/configured.*1234/i)).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: /delete credential/i }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("delete_provider_credential");
      expect(screen.getByText(/not configured/i)).toBeInTheDocument();
    });
  });

  it("runs Test Connection handshake and displays success or actionable failure", async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "get_provider_credential_status") {
        return {
          ...defaultStatus,
          configured: true,
          masked_preview: "••••1234",
        } satisfies ProviderCredentialStatus;
      }
      if (command === "get_platform_capabilities") {
        return defaultCapabilities;
      }
      if (command === "test_provider_connection") {
        return {
          success: true,
          message: "Connected to gemini-3.5-transcribe-live successfully.",
          error_kind: null,
        } satisfies ConnectionTestResult;
      }
      return null;
    });

    render(<OnboardingPanel />);

    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: /test connection/i }),
      ).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: /test connection/i }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("test_provider_connection", {
        candidateKey: null,
      });
      expect(
        screen.getByText(/connected to gemini-3\.5-transcribe-live/i),
      ).toBeInTheDocument();
    });

    invokeMock.mockImplementation(async (command: string) => {
      if (command === "test_provider_connection") {
        return {
          success: false,
          message: "Gemini rejected the Provider Credential (401 Unauthorized).",
          error_kind: "unauthorized",
        } satisfies ConnectionTestResult;
      }
      return null;
    });

    fireEvent.change(screen.getByLabelText(/provider credential/i), {
      target: { value: "AIzaSyBadKey9999" },
    });
    fireEvent.click(screen.getByRole("button", { name: /test connection/i }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("test_provider_connection", {
        candidateKey: "AIzaSyBadKey9999",
      });
      expect(screen.getByText(/401 unauthorized/i)).toBeInTheDocument();
    });
  });

  it("renders macOS permission checklist with deep-link buttons", async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "get_provider_credential_status") {
        return defaultStatus;
      }
      if (command === "get_platform_capabilities") {
        return {
          display_server: "mac_os",
          microphone_permission: "not_determined",
          accessibility_permission: "denied",
          supports_global_hotkey: true,
          supports_text_insertion: true,
          wayland_helpers: [],
          secret_service_available: true,
        } satisfies PlatformCapabilities;
      }
      return null;
    });

    render(<OnboardingPanel />);

    await waitFor(() => {
      expect(screen.getByText(/macos permissions/i)).toBeInTheDocument();
    });

    fireEvent.click(
      screen.getByRole("button", { name: /open microphone settings/i }),
    );
    expect(invokeMock).toHaveBeenCalledWith("open_permission_settings", {
      target: "microphone",
    });

    fireEvent.click(
      screen.getByRole("button", { name: /open accessibility settings/i }),
    );
    expect(invokeMock).toHaveBeenCalledWith("open_permission_settings", {
      target: "accessibility",
    });
  });

  it("renders Linux Wayland degraded capability status and Secret Service guidance", async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "get_provider_credential_status") {
        return {
          configured: false,
          masked_preview: null,
          secret_service_available: false,
          degraded_reason: "D-Bus Secret Service daemon not running",
          guidance:
            "OS Secret Service (libsecret) is unavailable. Start gnome-keyring or KWallet.",
        } satisfies ProviderCredentialStatus;
      }
      if (command === "get_platform_capabilities") {
        return {
          display_server: "linux_wayland",
          microphone_permission: "not_applicable",
          accessibility_permission: "not_applicable",
          supports_global_hotkey: false,
          supports_text_insertion: false,
          wayland_helpers: [],
          secret_service_available: false,
        } satisfies PlatformCapabilities;
      }
      return null;
    });

    render(<OnboardingPanel />);

    await waitFor(() => {
      expect(screen.getByText(/linux wayland/i)).toBeInTheDocument();
      expect(screen.getByText(/fallback delivery/i)).toBeInTheDocument();
      expect(screen.getByText(/gnome-keyring or kwallet/i)).toBeInTheDocument();
    });
  });
});
