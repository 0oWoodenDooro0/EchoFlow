import { describe, expect, it } from "vitest";
import tauriLibRsSource from "../../src-tauri/src/lib.rs?raw";
import tauriTsSource from "./tauri.ts?raw";

function snakeToCamel(name: string): string {
  return name.replace(/_([a-z0-9])/g, (_, ch: string) => ch.toUpperCase());
}

/**
 * Extracts `#[tauri::command]` functions and their expected Tauri v2 camelCase
 * IPC argument names (excluding injected `State<...>` and `AppHandle` params).
 */
function parseRustCommands(source: string): Map<string, string[]> {
  const commands = new Map<string, string[]>();
  const cmdRegex =
    /#\[tauri::command\]\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)\s*\(([\s\S]*?)\)\s*(?:->|\{)/g;

  for (const match of source.matchAll(cmdRegex)) {
    const fnName = match[1];
    const rawParams = match[2].trim();
    const expectedArgs: string[] = [];

    if (rawParams.length > 0) {
      // Split top-level comma-separated parameters (ignoring commas inside <>).
      const params: string[] = [];
      let depth = 0;
      let current = "";
      for (const ch of rawParams) {
        if (ch === "<") depth++;
        else if (ch === ">") depth--;
        if (ch === "," && depth === 0) {
          params.push(current.trim());
          current = "";
        } else {
          current += ch;
        }
      }
      if (current.trim().length > 0) {
        params.push(current.trim());
      }

      for (const param of params) {
        const colonIdx = param.indexOf(":");
        if (colonIdx === -1) continue;
        const name = param
          .slice(0, colonIdx)
          .replace(/^mut\s+/, "")
          .trim();
        const ty = param.slice(colonIdx + 1).trim();
        if (
          ty.includes("State<") ||
          ty.includes("AppHandle") ||
          ty.includes("Window") ||
          ty.includes("WebviewWindow")
        ) {
          continue;
        }
        expectedArgs.push(snakeToCamel(name));
      }
    }

    commands.set(fnName, expectedArgs.sort());
  }

  return commands;
}

/**
 * Extracts `invoke<...>("command_name", { ... })` calls and their top-level
 * payload keys from `tauri.ts`.
 */
function parseFrontendInvokes(
  source: string,
): Array<{ command: string; keys: string[] }> {
  const invokes: Array<{ command: string; keys: string[] }> = [];
  const invokeRegex =
    /invoke(?:<[^>]+>)?\(\s*"([^"]+)"(?:\s*,\s*\{([\s\S]*?)\})?\s*\)/g;

  for (const match of source.matchAll(invokeRegex)) {
    const command = match[1];
    const body = match[2];
    const keys: string[] = [];

    if (body) {
      for (const rawEntry of body.split(",")) {
        const entry = rawEntry.trim();
        if (!entry) continue;
        const keyMatch = entry.match(/^(\w+)\s*(?::|$)/);
        if (keyMatch) {
          keys.push(keyMatch[1]);
        }
      }
    }

    invokes.push({ command, keys: keys.sort() });
  }

  return invokes;
}

describe("Tauri v2 IPC command contract", () => {
  const rustCommands = parseRustCommands(tauriLibRsSource);
  const frontendInvokes = parseFrontendInvokes(tauriTsSource);

  it("finds Rust commands and frontend invoke calls", () => {
    expect(rustCommands.size).toBeGreaterThan(0);
    expect(frontendInvokes.length).toBeGreaterThan(0);
  });

  it("matches every frontend invoke command name and camelCase payload keys against Rust", () => {
    for (const { command, keys } of frontendInvokes) {
      expect(
        rustCommands.has(command),
        `Frontend invokes unknown Tauri command "${command}"`,
      ).toBe(true);

      for (const key of keys) {
        expect(
          key,
          `Payload key "${key}" for "${command}" must be camelCase (Tauri v2 default)`,
        ).not.toContain("_");
      }

      const expectedKeys = rustCommands.get(command) ?? [];
      expect(
        keys,
        `Payload keys for "${command}" do not match Rust #[tauri::command] params`,
      ).toEqual(expectedKeys);
    }
  });
});
