# Friend remote

Friend remote gives one friend a web page where they can disconnect and reconnect the Roblox accounts you choose, from any browser. They need nothing installed. Nothing else on your PC is reachable through it.

You need a free Tailscale account and about 2 minutes. Your friend installs nothing. RMI uses Tailscale Funnel to put a web address on the internet that points at a small server inside RMI.

## Set it up

Open Settings, Friend remote. Until a link exists, RMI shows three steps. Each step is tagged "Done", "Needs you" or "Waiting", and it updates by itself every few seconds.

1. **Install Tailscale.** Click "Download Tailscale" (it opens https://tailscale.com/download), run the installer and finish it. This step is done when RMI finds the Tailscale command line tool.
2. **Sign in to Tailscale.** A free account works. This step is done when Tailscale reports it is running.
   - Windows: click the Tailscale icon in the system tray near the clock (it may hide behind the ^ arrow), choose Log in, and sign in.
   - macOS: click the Tailscale icon in the menu bar, choose Log in, and sign in.
   - Linux: run `sudo tailscale up` in a terminal and open the link it prints.
   - Still waiting? Open Tailscale and check that it says Connected.
3. **Turn on friend link.** Click "Turn on friend link". RMI runs Tailscale for you and saves the address it gets. If it fails, RMI shows Tailscale's message. Click the button again, or use "I have my own address".

If Tailscale needs your OK to share a link (Funnel or HTTPS is switched off for your account), RMI shows "Allow Funnel for your Tailscale account". Click "Open Tailscale page", approve it there, then come back and click "Try again". RMI opens the link Tailscale gives it. You only do this once per account.

If you already have your own HTTPS address that points at RMI, click "I have my own address" and enter it by hand.

## Choose which accounts a friend can use

In Settings, Friend remote, under "Accounts your friend can control", pick the accounts the friend may control. They see only those. An account you did not pick looks like it does not exist.

The friend can do two things: Disconnect and Reconnect. They cannot see cookies or passwords or open anything else in RMI. Each account accepts one Disconnect or Reconnect every 20 seconds.

## Give your friend access

Once the link exists, Settings, Friend remote shows "What to send your friend": the link, the username and the password, each with its own "Copy" button.

- Link, for example `https://rmi.example.ts.net`. Send it exactly as shown.
- Username. Default `friend`, not case sensitive. You can change it in the "Friend username" field.
- Password. RMI generated it (20 characters). Click the eye to show it.

Two buttons help:

- **Copy invite** copies a ready message with the link and the username. It tells your friend the password comes separately. The password is never in it.
- **Open the page** opens the link in your browser, so you see what your friend sees.

Send the invite first, then the password in a separate message. The friend opens the link, logs in, and sees the accounts you allowed.

### New password

"New password" makes a fresh password and signs every browser out at once. Use it if the password leaked, or when you stop trusting a device. Send the new password to your friend.

## Your own login

You can add a second login for yourself, for example to check on your accounts from your phone. It sees every account, not only the ones you shared, and which computer each one runs on.

1. In Settings, Friend remote, type a name in "Your own login" (1 to 32 characters). It must differ from the friend username, or RMI shows an error and does not save it.
2. Click Save. A password line appears: click the eye to show it or "Copy" to copy it.
3. Open the same link and log in with that name and password.

Leave the field empty to turn it off. Give this login only to yourself. "New password" only changes the friend's password. Your own password stays the same.

## Letting your friend add accounts

Switch on "Let your friend add accounts" in Settings, Friend remote (off by default). Your friend then sees an "Add account" button on the friend page and can add their own Roblox accounts while you are away. They never type a password or cookie into the page.

The page shows a 6-character code. Your friend:

1. Opens Roblox on a phone or PC where they are logged in to that account.
2. Goes to Settings, then Quick Log In (on mobile, Settings is in the "..." More menu).
3. Types the code, checks the device and confirms.

RMI then signs that account in from your PC with Roblox Quick Log In, adds it to your vault and shares it with your friend, so it shows in their list right away. You get a notice in RMI, and the account is marked "Added by friend" in Settings, Friend remote.

Limits:

- At most 10 accounts added by the friend, and 5 new codes per 10 minutes.
- A code lasts at most 5 minutes. Closing the sheet or "Cancel" ends it.
- One code at a time. Asking again while one is waiting shows the same code, and that does not count toward the 5.
- If the account is already in your vault but not shared with your friend, nothing changes: the page says "This account can't be added. Ask the owner." (it does not say the account is on your PC). If it is already shared, its session is refreshed and your settings for it (group, destination, pause) stay as they are. An account your friend added that you later un-shared stays refused and still counts toward the 10.
- Turning the switch off, turning the friend remote off or pressing "New password" ends a waiting code at once. It also stops an add that is already signing in: the check runs again right before the account is written to your vault.
- If the sign-in has not finished 60 seconds after your friend confirmed, the page shows that it failed and a new code can be made.
- If Roblox asks for an extra check during the sign-in, the add fails and the page says so. Accounts with 2-step verification always get that check, so they can not be added this way.
- Roblox also checks where a login comes from. The sign-in happens on your PC while your friend approves it from their device, so it usually only works when your friend is on the same network as your PC or close to it. From far away Roblox can refuse the login or sign the account out later.
- A friend-added account is a full Roblox session in your vault: you can use it like one of your own. Your friend should only add an account if they trust you with it.
- When an add is refused or fails after Roblox already signed your PC in, RMI signs that new session out again so it does not stay live. An account that was added or refreshed is never signed out.

The new account's session lives only in your encrypted vault on this PC. The friend page never receives it, RMI never logs it, and the page shows only the code as text (it loads nothing from Roblox).

To remove an account your friend added, remove it in Accounts like any other. To take away only their access, switch it off under "Accounts your friend can control".

## Turn it off

Switch off "Let a friend control accounts" in Settings, Friend remote. RMI stops its server and removes the Tailscale Funnel entry it made. The old link stops working. RMI also removes that entry when you change the friend port or clear the public address, so nothing else that later uses the old port is put on the internet.

To stop sharing for good, press "Turn off friend link" in Settings, Friend remote (shown when the address is one RMI made with Tailscale). RMI removes its Funnel entry, clears the address and turns the friend remote off, and you are back at the setup steps.

RMI only removes an entry whose single handler (`/`) points at the friend server's own port. Entries you made yourself are never touched. If Tailscale is not running when you turn it off, RMI notes it in its log and the entry stays; run `tailscale funnel status` to check and `tailscale funnel --https=<port> off` to remove it.
## Troubleshooting

**The link does not open.** Check that RMI is running and the friend link is on. The PC must be awake and online. Tailscale must be running on your PC. A new Funnel address can take a minute to start working.

**"Wrong username or password."** Check the username and password in your message. Ask the owner for a new password if it still fails.

**"Too many tries. Wait 1 minute, then try again."** Ten wrong logins from one address in 60 seconds triggers a one minute wait. Wait, then log in with the right username and password. Use "New password" if you lost it.

**RMI is locked.** The friend page says the owner's vault is locked. Unlock RMI. While the vault is locked, the friend cannot act.

**Port 443 is already used.** If another Tailscale Serve or Funnel entry uses 443, RMI picks 8443, then 10000, and the link ends in `:8443` or `:10000`. Give your friend the link exactly as shown. If all three ports are used, RMI says so and you need to free one in Tailscale.

**Tailscale not found.** RMI looks on your PATH, then in the default install folder (`C:\Program Files\Tailscale` on Windows, `/Applications/Tailscale.app` on macOS). Reinstall Tailscale in the default place.

## Security notes

- The friend server listens only on `127.0.0.1`. Tailscale Funnel is the only thing that exposes it, and RMI never replaces a Serve or Funnel entry that points somewhere else.
- Without logging in, a visitor can load only the login page and its static files. Everything else needs the token you get by logging in, and a wrong or unknown account answers the same as one you did not allow.
- Login attempts are limited to 10 failures per client address per 60 seconds, then a "Too many tries" answer.
- The password and token are stored encrypted with your OS secret store, in RMI's data folder.
- The page sends a strict Content Security Policy (no inline scripts, no framing) and only answers for your public address or loopback.
- Anyone with the link, username and password can disconnect and reconnect the allowed accounts. Share them with one person you trust.
