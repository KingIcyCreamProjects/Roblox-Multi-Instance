import type { ConsoleLine } from "@/lib/ipc";

export const DOCS_URL =
  "https://github.com/KingIcyCreamProjects/Roblox-Multi-Instance/blob/main/docs/AUTOMATION.md";

/** Only the last 4 characters stay visible. */
export const maskToken = (tok: string) => (tok ? `••••${tok.slice(-4)}` : "");

export function filterLines(lines: ConsoleLine[], q: string): ConsoleLine[] {
  const n = q.trim().toLowerCase();
  return n ? lines.filter((l) => l.text.toLowerCase().includes(n)) : lines;
}

export const fmtTime = (ms: number) => new Date(ms).toLocaleTimeString([], { hour12: false });

export const wsUrl = (port: number, tok: string) =>
  `ws://127.0.0.1:${port}/control?token=${tok}&account=<name>`;
// Windows PowerShell 5.1 aliases `curl` to Invoke-WebRequest; curl.exe is the real curl there
export const curlCmd = (port: number, tok: string) =>
  `${/Windows/.test(navigator.userAgent) ? "curl.exe" : "curl"} -H "Authorization: Bearer ${tok}" http://127.0.0.1:${port}/v1/accounts`;
