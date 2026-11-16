/** A line, or one with a "Show me" button. `to`: a tab, or a settings row (its catalog
 * path, e.g. "watcher.reconnect"). */
export type Line = string | { text: string; to: string };

/** One release in the update log. Newest first; add a new entry on top with every release. */
export type Update = {
  version: string;
  /** YYYY-MM-DD */
  date: string;
  title: string;
  summary: string;
  added?: Line[];
  improved?: Line[];
  fixed?: Line[];
};

export const UPDATES: Update[] = [
  {
    version: "1.0.0",
    date: "2026-11-16",
    title: "Public release",
    summary:
      "RMI is now open source, with installers for Windows, macOS and Linux. Bug reports and pull requests are welcome on GitHub.",
    added: [
      {
        text: "Remote access: see and control your other computers from the Devices tab, over Tailscale. Off until you switch it on.",
        to: "devices",
      },
      {
        text: "Friend remote: one link lets a friend disconnect and reconnect the accounts you pick.",
        to: "friend.enabled",
      },
      "Linux: launch, multiple clients and auto reconnect through Sober.",
      "Friend remote shows when an account is in game on another computer, and has Select all and Clear all.",
      "Import an encrypted vault export from Settings.",
    ],
    improved: [
      {
        text: "File transfer in remote access is off until you turn it on.",
        to: "remote.files",
      },
      {
        text: "Letting a friend add accounts now explains its limits: no 2-step verification, and Roblox checks where the login comes from.",
        to: "friend.canAdd",
      },
      "Smoother remote access picture with H.264 video on Windows and macOS.",
    ],
  },
];
