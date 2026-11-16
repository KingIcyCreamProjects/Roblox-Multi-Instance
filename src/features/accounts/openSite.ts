import { toast } from "@/components/Toasts";
import { call, errMessage } from "@/lib/ipc";

/** roblox.com signed in as this account, in an ephemeral webview (account_open_site). */
export const openSite = (accountId: string) =>
  void call("account_open_site", { accountId, url: "https://www.roblox.com/home" }).catch((e) =>
    toast(errMessage(e)),
  );
