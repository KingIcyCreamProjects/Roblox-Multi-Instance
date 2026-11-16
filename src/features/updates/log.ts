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
      "RMI runs many Roblox accounts on one computer, from one encrypted vault. This is the first public release: open source, with installers for Windows, macOS and Linux. Here is what is in it.",
    added: [
      {
        text: "Home: press Play to launch the accounts you used last, and see every running client.",
        to: "home",
      },
      {
        text: "Accounts: your accounts in an encrypted vault, sorted into groups. Each account and group remembers where it plays.",
        to: "accounts",
      },
      {
        text: "Discover: search games, browse servers by players, ping and region, and keep favorites.",
        to: "discover",
      },
      {
        text: "Automation: a local API for your own scripts. Off until you enable it.",
        to: "automation",
      },
      {
        text: "Devices: see and control your other computers over Tailscale. Off until you switch it on.",
        to: "devices",
      },
      {
        text: "Friend remote: one link lets a friend disconnect and reconnect the accounts you pick.",
        to: "friend.enabled",
      },
      {
        text: "Auto-reconnect: a client that got disconnected rejoins without touching your mouse or keyboard.",
        to: "watcher.reconnect",
      },
      {
        text: "Anti-AFK keeps accounts in game while you are away.",
        to: "antiAfk.enabled",
      },
      "Grid Arrange tiles every Roblox window on a monitor, in the order you pick.",
      "Linux: launch, multiple clients and auto-reconnect through Sober.",
    ],
    improved: [
      {
        text: "File transfer in remote access is off until you turn it on.",
        to: "remote.files",
      },
      {
        text: "Letting a friend add accounts explains its limits: no 2-step verification, and Roblox checks where the login comes from.",
        to: "friend.canAdd",
      },
      "The tutorial explains friend remote in more detail, and you can skip it from any step.",
      "Friend remote shows when an account is in game on another computer, and has Select all and Clear all.",
      "Smoother remote access picture with H.264 video on Windows and macOS.",
    ],
  },
];
