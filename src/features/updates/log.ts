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
    version: "1.0.3",
    date: "2026-11-17",
    title: "Updates install themselves",
    summary:
      "When you open RMI and a new version is out, it now installs it and restarts by itself, the way a game launcher does.",
    added: [
      {
        text: "Install updates automatically: on by default. RMI only updates when it opens, never in the middle of a session. Switch it off to install from Settings yourself.",
        to: "updates.install",
      },
    ],
    improved: [
      "Launch on login still launches your accounts when RMI updates itself on the way up, and never restarts an account that is already running.",
    ],
  },
  {
    version: "1.0.2",
    date: "2026-11-17",
    title: "Run here, and accounts that start with your computer",
    summary:
      "Move a running account from one of your computers to another with one click, and have RMI launch your accounts by itself when it starts on login.",
    added: [
      {
        text: "Run here: an account running on a computer you paired shows where it runs. Click Run here and that computer closes it while this one launches it. It works for one account, a selection, or everything on a computer from the Devices tab. Both computers need this version.",
        to: "devices",
      },
      {
        text: "Launch on login: name the accounts RMI should launch when it starts with your computer, each into its saved destination.",
        to: "startup.accounts",
      },
    ],
    improved: [
      "Computers you paired now tell each other which accounts they run in both directions, so one pairing is enough.",
    ],
  },
  {
    version: "1.0.1",
    date: "2026-11-16",
    title: "Fixes, and an easier friend remote",
    summary:
      "A full pass over the whole app found and fixed more than 90 problems. Friend remote is easier to set up, and its phone page shows more.",
    added: [
      {
        text: "Friend remote setup walks you through each step with links, then shows exactly what to send, with a Copy invite button.",
        to: "friend.enabled",
      },
      {
        text: "Your own login for the phone page: it sees every account and which computer each one runs on.",
        to: "friend.ownerUsername",
      },
      "The phone page fits about twice as many accounts on a screen, has a summary line, and says when an account is starting or runs on another computer.",
      "Computers you paired tell each other which accounts they run, so the phone page can name the computer.",
    ],
    improved: [
      "Settings that do nothing on your system are hidden or dimmed instead of looking like they work.",
      "Discover: Reserve and join blocks a second Play while it works, loading regions can be restarted, and missing thumbnails are retried.",
      "Play bar: server links, links with a language in them and ro.blox.com links now save and launch correctly.",
      "A saved cookie that belongs to another Roblox user is flagged instead of trusted.",
      "macOS: after an update, RMI explains how to give Accessibility access back.",
    ],
    fixed: [
      "Windows: all Roblox clients but one could close when the computer was under heavy load.",
      "Auto-reconnect: a disconnect skipped by the hourly limit is retried later, a crash after a rejoin is relaunched, and it works on Linux without passt.",
      "Changing the vault password asks for the system check first, like exporting does.",
      "A new vault no longer removes the backups of an older one, and recovery works for a damaged vault that opens without a password.",
      "Friend remote: the link survives a port change, and Tailscale Funnel is switched off when RMI can not use its port.",
      "Remote access: a finished file transfer no longer blocks the next one, and long file names in any language upload.",
      "Saved window positions are kept for minimized windows and ignored for a monitor that is gone.",
      "Anti-AFK works with a maximized window in front and stops when you come back.",
      "After installing an update, RMI opens its window instead of hiding in the tray.",
      "Many smaller fixes in Accounts, Grid Arrange, import, the tray menu and the texts.",
    ],
  },
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
        to: "automation.enabled",
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
