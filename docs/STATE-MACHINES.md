# Implicit state machines

Audit date: 2026-11-06. Working tree at `62d3177` plus the uncommitted edits present that day.

> **Historical snapshot.** Later work is not covered: the `Automation.handed` set (Run here hand-over and take-back), the watcher reconnect task and its `Track` fields, the toast `locked` flag, the idle auto-lock that waits while RMI-launched clients run, and the relaunch engine's `handback` and `prev_own` fields (H3 lists the 2026-11-06 states). File:line references have drifted; search by function name.

**What counts.** An implicit state machine is anything that moves through phases over time without one explicit state type that owns its transitions. That covers:
- bools, Options, refs or nulls that together encode a phase
- status strings compared in several places
- retry, poll and timer protocols
- UI step flows
- the same state kept in both Rust and React

**SEMI** means an enum exists, but its transitions are spread out with no single transition function.

**Severity labels.**
- **REACHABLE**: a concrete path to the bad state exists.
- **THEORETICAL**: the encoding allows it, but no path was found.

**How it was checked.** Six read-only reviews ran, one per slice. Entries marked **✔** were re-checked by hand against the code. Unmarked entries come from a single review pass.

**Totals.** 10 HIGH, 29 MED and 46 LOW machines, plus 6 patterns that cut across them.

---

## Cross-cutting patterns (fix once, many machines improve)

**X1. Locking unmounts the Shell, so event listeners vanish while locked.**
**Fixed (phase 1):** `useLaunchEvents`, `useAccountEvents`, `useSecretToast` (and `useStateSync`) now mount in `App.tsx`, so they survive a lock; toasts name an account only via `namesHidden(qc)` (`useAccounts.ts`: locked or privacy on hides names); the `App.tsx` lock-effect launch reset is gone. H5 resync and D1 stream terminal events are still open.
- `App.tsx:141` swaps the Shell for Unlock. That removes the listeners for:
  - `launch://*` (Shell)
  - `signin://result` (`useAccountEvents`, `Shell.tsx:112`)
  - `automation://notice` (`useSecretToast`, `Shell.tsx:111`)
- Each affected machine copes differently, or not at all:
  - launch batch: reset by hand at `App.tsx:59` (H5)
  - sign-in: not handled (H2)
  - automation notice: dropped (P2)
  - server streams: no terminal event on cancel, masked only by the unmount (D1)

**X2. Rust→React mirrors are one-way.**
**Fixed (phase 1):** Rust `commands::state_changed(app, &["key", ...])` emits `state://changed`; `src/app/useStateSync.ts` (mounted in App) invalidates each named root key. Wired: `["autoToken"]` (every token notice), `["autoStatus"]` (startup start, `settings_set` reconcile), `["settings"]` (shell autostart mirror, tray-hint flag), `["update"]` (every update check; replaced `update://available`). Phase 2 automation wired `["snap"]` (hub changes, engine resets, lock) and `["autoStatus"]` (`Automation::status_changed`: bind, stop, task death; covers start/stop/crash/rebind failure). Still to wire: `["roblox"]`, `["vault"]` (unlock/create), `["pin", id]`.
**Fixed (phase 2, app core):** `["settings"]` from `client_settings_set`, `multi_set`, `cookie_lock_set` (`settings_set` callers sync via the mutation; `import_commit` callers invalidate in `ImportSheet`); `["roblox"]` from `settings_set` when `multi` changes, `multi_set`/`cookie_lock_set`, and the launcher when the install (version folder or bootstrapper) differs from the last one seen (`commands::install_changed`).
- `queryClient.ts:5` sets `staleTime: Infinity`. Caches change only on specific events or on the app's own invalidations.
- Any Rust change that emits no event leaves the UI stale. Affected caches:
  - `["autoStatus"]` (H4)
  - `["autoToken"]` (A2)
  - `["snap"]` (A3)
  - `["settings"]` (P1)
  - `["roblox"]` (L2)
  - `["update"]` (H10)
  - `["vault"]` (H9)
  - `["pin", id]`

**X3. Rust error text is used as UI state.**
**Fixed (phase 1):** Rust now returns codes `VAULT_LOCKED`, `VAULT_WAS_LOCKED`, `WRONG_PASSWORD`, `VAULT_DAMAGED`, `PREVIEW_EXPIRED` (plus existing `REAUTH_REQUIRED`); `src/lib/ipc.ts` has `hasCode`/`errCode` (exact match) and `CODE_TEXT`, which `errMessage`/`codeText`/`errText` use to show the same English text. Locked detection, `Unlock` recovery, `ImportSheet` expiry and reauth all use `hasCode`; `VAULT_WAS_LOCKED` now refetches vault status too.
- Pairs where React matches Rust's wording:
  - "The vault is locked.": `commands.rs:528` ↔ `ipc.ts:475`
  - "Wrong password." and "This vault file is damaged.": `vault/mod.rs:11,13` ↔ `useVault.ts:26-27` and `Unlock.tsx:133`
  - import `EXPIRED`: `import/cmds.rs:28` ↔ `ImportSheet.tsx:28`
  - reauth: `useAccounts.ts:115` matches a substring, though the contract says exact match
- "The vault was locked." (`commands.rs:529`) matches none of them.
- Rewording any of these Rust strings silently breaks a transition. Error codes already exist in the app (`hasCode(e, "PIN_REQUIRED")`), so the fix pattern is available.

**X4. The lock-generation guard is re-implemented at each call site.** Each copy snapshots `lock_gen`, does its work, then compares. There are about 15 copies:
**Fixed (phase 1):** `commands::Gen` (`Gen::now(&lock_gen)`, `.current(&lock_gen)`, `.check(&lock_gen)` → `Err(WAS_LOCKED)`) replaces every listed copy plus `commands.rs` (`insert_if_current`, `import_impl`, `unlock_impl`); `mutate` takes a `Gen`. `vault::take_for_lock` still bumps when already locked, so Lock cancels an unlock/create in progress (user decision "lock wins", see H8); a Lock with nothing in progress changes nothing else.
- `accounts/cmds.rs:35`
- `accounts/sync.rs:56,99,167,201,218,235`
- `accounts/signin.rs:115,216`
- `import/cmds.rs:175,226,523,610`
- `tools/mod.rs:158,178,402`
- `launch/launcher.rs:332`
- `watcher/mod.rs:273`
- `automation/backend.rs:167,182`

Also, `lock()` bumps the generation even when the vault is already locked (`vault/mod.rs:118`).

**X5. `SessionState` is written from many sites with no transition function.**
**Fixed (phase 1):** `accounts::set_session(p, id, to, about_cookie)` is the single writer (drops verdicts about a replaced cookie, never moves to `Pending`); `accounts::cmds::expire_session(st, id, cookie)` marks a 401. Used by `upsert`, `apply_checks`, tools `store_cookie`/`expire`, launcher `expire`, import, and Discover (`servers_regions`, `servers_stream`, `servers_find_player` on a 401).
- Writers:
  - accounts: `mod.rs:288,301`, `sync.rs:276-278`
  - tools: `tools/mod.rs:68-79,190-238`
  - import
- Discover never marks an account expired on a 401 (`discover/mod.rs:533`, server streams).

**X6. Busy guards live in component state, or are missing.**
- Affected flows: updater (H10), PIN prompt (H1), reserve-then-join (D7), region loads (D3), hero Play and OnlineNow (H5), every "Sign in again" (H2), token regenerate (A2), automation toggles.
- A remount or a second entry point bypasses the guard.

**Fixed (phase 2, per flow):** guards moved to Rust or module stores: updater `UPDATE_BUSY` (H10), PIN prompt (H1), reserve-then-join `RESERVE_BUSY` (D7), region loads (D3), launch batch + hero Play/OnlineNow (H5), sign-in `SIGNIN_BUSY` (H2), vault create `VAULT_BUSY`, import steps (V5), relaunch toggle (automation LOW). See each item's **Fixed:** line.

---

## HIGH

### H1. PIN unlock then retry (Tools sheet): IMPLICIT ✔
- **Where:** `src/features/tools/ToolsSheet.tsx:55-95, 157-166, 243-300`
- **Encoding:**
  - `pin` state plus `pinRef`
  - `Pending {retry, cancel}` closures that hold the original op's promise
  - PinPrompt `busy`/`err`/`pin`
  - query `["pin", id]`
- **States:** idle → op running → awaiting PIN → unlocking → retrying, or cancelled. There is also a hidden-pending state when `ready` turns false.
- **Problems:**
  - **REACHABLE ✔: a cancelled op still runs.**
    - Cancel stays enabled while unlocking (`:298`).
    - Path: click Unlock, then Cancel, close the sheet or switch tab before the reply arrives.
    - `setPin(null)` rejects the caller, but when `tool_pin_unlock` resolves, the stale `onDone` (`:160-164`) calls `pin.retry()`.
    - That re-runs the cancelled op (password change, sign-out-others, unblock-all) and throws its result away.
  - **REACHABLE: no PIN prompt for single-account batch tools.**
    - Display name, privacy, friends and block return `PIN_REQUIRED` as a row message (`tools/mod.rs:128-137`), not as a thrown error.
    - So a single selected account never gets the prompt. This contradicts `ToolsSheet.tsx:72` and `docs/contracts/tools.md`.
  - **THEORETICAL: the selection empties while the sheet is open.**
    - `open` turns false without `onOpenChange`, so the pending promise and `pinRef` leak and `toolsOpen` stays true.
  - **Mirror only:** Roblox's unlock window is copied into the query cache plus a local countdown. An unlock or re-lock made elsewhere never shows up.
- **Why HIGH:** the only path found that executes an action the user cancelled.
- **Fixed:** `onDone` retries only if its entry is still the pending one (`pinRef.current === pin`), so Cancel/close/tab switch during unlock never re-runs the op; a single-account batch row `PIN_REQUIRED` throws into `guard` (prompt + retry, `useTool.tsx` `useBatchAction`); the pending entry is cancelled when its account scope goes (selection change, session lost, unmount) and an emptied selection resets `toolsOpen` (`ToolsSheet.tsx`); Rust emits `state_changed(["pin"])` on every `PIN_REQUIRED` (`tools/mod.rs` `fail`, `Change::Pin`), so the chip refetches after a re-lock.

### H2. Sign-in lifecycle and `signinWaiting`: IMPLICIT ✔
- **Where:**
  - Rust: `accounts/signin.rs:115-243`
  - React: `app/store.ts:23,47`, `accounts/useAccounts.ts:43,155-157`, `AddSheet.tsx:92,148`
- **Encoding:**
  - Rust has no session object. The phase is the combination of: the `login-<uuid>` window exists, a detached 1 s poll task is running, and a captured lock generation.
  - Temp-dir cleanup retries 20 × 500 ms.
  - React has one global bool, set only by the result event.
- **States:** idle → window open and polling → cookie found → finishing (auth + save) → ok or error → window destroyed → dir cleanup retrying. A window the user closes reads as "cancelled".
- **Problems:**
  - **REACHABLE ✔: Add button stuck after a lock.**
    - Start sign-in, then lock. `vault::lock` destroys the window and the Shell unmounts, taking the listener with it (X1).
    - The lock effect (`App.tsx:53-60`) resets the launch state but not `signinWaiting`.
    - After unlock, the Add sheet's sign-in button stays disabled until the app restarts.
  - **REACHABLE: several windows, one bool.**
    - Rust has no single-flight guard.
    - "Sign in again" is unguarded at `Inspector.tsx:575`, `AccountMenu.tsx:202`, `useTool.tsx:60` and `ImportSheet.tsx:467`.
    - The first result clears `signinWaiting` while other windows are still open.
  - **REACHABLE: full login repeated after a network error.**
    - If auth fails after the cookie was found (`signin.rs:219-221`), the window is destroyed anyway.
    - The user must redo the whole login, 2-step included.
  - **REACHABLE: wrong or lost result.**
    - A lock reports "Sign-in cancelled."
    - A webview reload resets the bool while the window stays alive.
- **Fixed:** Rust single flight (`Flight` guard in `accounts/signin.rs`, held until the result is emitted): a second `signin_start` from any entry point focuses the open window and returns `SIGNIN_BUSY` (React keeps `signinWaiting`, toasts it; `useAccounts.ts`, `ipc.ts`, `i18n.ts`); a window gone after a lock reports `VAULT_WAS_LOCKED` (no toast) instead of "cancelled"; a transient auth failure (network, 429, 5xx) keeps the signed-in window and retries; result errors go through `codeText`. Lock-stuck button already fixed by phase 1 (listener in App).

### H3. Auto-relaunch engine: SEMI ✔
- **Where:**
  - `automation/relaunch.rs:83-97`: the `St` struct with 9 fields, transitions in a 4-way if chain
  - callers: `automation/mod.rs:394-398`, `relaunch` fn at `relaunch.rs:412`, `Loop::step` at `relaunch.rs:491`
- **States:** disabled, watching, pending, rearmed, inflight, quiet, stopped, unknown-edge (after a reset).
- **Problems:**
  - **REACHABLE ✔: `quiet` gets stuck.**
    - `quiet` clears only on a rise (`:132-133`). `reset()` sets `prev_up = None` (`:256`).
    - After a server stop/start or a token regenerate, the account comes back without a "rise". It stays quiet, so its next real crash is ignored.
  - **REACHABLE ✔, narrow: double launch from a stale `rearmed`.**
    - `locked()` and `quiet()` clear `pending` but not `rearmed` (`:213-229`).
    - A later exit arms a normal pending while `rearmed` is still true. An RMI launch then skips the cancel (`:144`), so two launches happen.
    - This needs the client to exit without reconnecting in between.
  - **REACHABLE: `stopped` never clears.**
    - It clears only when a tick sees enabled go from false to true (`:122`).
    - Ticks run only while enabled, unlocked and serving. Toggling off and on while stopped or locked, or within 1 s, never clears it.
    - This contradicts `docs/AUTOMATION.md:242`.
  - **REACHABLE ✔: a pending relaunch survives a reset.**
    - Spawned relaunch tasks (`:348`) have no cancel token and follow their batch for up to 600 s.
    - The `finished(rearm)` guard (`:235`) checks fields that `reset()` doesn't touch, and `retry()` (`:243-249`) has no guard at all.
    - So a pending relaunch survives a server stop or token regenerate and fires after the next start.
    - This contradicts `AUTOMATION.md:252-254` and the doc comment on `reset`.
  - **REACHABLE: unknown accounts trip the cooldown.**
    - A Launch for a hub name with no saved account still counts toward the cooldown (`:197-199`); it is not undone at `:555`.
    - Five of them trip `stopped`.
- **Fixed:** `relaunch.rs`: quiet clears on any up-after-not-up (also after `reset`'s forgotten edge); `pending` + `rearmed` merged into `pending: Option<(Instant, rearm)>` so every clear drops both; `stopped` clears via `Engine::resume`, fed by `Hub::take_reenabled` (`ws.rs`, recorded by `relaunch.set`/`set_relaunch` on off→on, drained every loop tick, even locked or not serving); `reset` bumps `Engine::epoch`, `Relaunch`/`Fin` carry it, `relaunch()` checks it under the engine lock before `start_batch`, `finished`/`retry` drop re-arms from an old epoch; unknown account uses `retry(.., None, ..)` (uncounted).

### H4. Automation server lifecycle: IMPLICIT
- **Where:**
  - `automation/mod.rs:167-173, 223, 401-406, 427-498`
  - `lib.rs:134, 356-381`
  - `commands.rs:325-334`
  - `shell.rs:160-181`
- **Encoding:** `tokio::Mutex<Option<Running>>`, plus per-task `is_finished()`. `serving()` uses `try_lock`, so "busy" reads as stopped.
- **States:** stopped, up, degraded (one task dead), busy, stopping (grace period, then abort).
- **Problems:**
  - **REACHABLE: servers bind after the user disabled automation.**
    - Startup decides `start_automation` once (`lib.rs:134`), awaits the keyring (`:360-366`), then calls `start()` without re-checking the setting.
    - If the user disables automation in the meantime, `reconcile` does nothing because `run` is `None` (`mod.rs:470-472`), and the servers bind anyway.
    - The same check-then-act gap exists in `automation_start` (`commands.rs:330-333`) and `resume` (`shell.rs:177`).
  - **THEORETICAL: degraded is never healed.** `reconcile` compares only the config (`:474`).
  - **REACHABLE: stale status page.**
    - `["autoStatus"]` gets no event on bind, stop, abort or task death.
    - The page can show "Stopped" while the servers run, for example during the startup keyring delay.
- **Why HIGH:** a network listener can run while the setting says off.
- **Fixed:** `automation/mod.rs`: `start()` checks `automation.enabled` under the run lock (covers startup, `automation_start`, `shell::resume`; `AUTOMATION_OFF`); `reconcile` also rebinds a degraded run (`!r.up()`); `Automation::status_changed` (`Notify`) fires on bind, shutdown and any serve task ending, and `lib.rs` turns it into `state_changed(["autoStatus"])`. `serving()` busy = keep last answer (already).

### H5. Front-end launch batch (React mirror of the Rust batch): IMPLICIT ✔
- **Where:**
  - `playbar/usePlay.ts:27-42, 58-99, 106-107, 125-141, 156`
  - `PlayBar.tsx:456-457, 506`
  - `App.tsx:59`
  - `Home.tsx:132-135, 253`
- **Encoding:**
  - `batch {id | null, ids, done | null, token}`, plus `states[batchId][acct]` and `dones[batchId]`
  - a 1200 ms settle timer and a 10-minute stuck timer
  - `running = !!batch`
- **States:** idle (`batch` null), starting (`id` null), running, settling, plus orphaned `states`/`dones` entries.
- **Problems:**
  - **REACHABLE ✔: the UI shows idle while the Rust batch keeps running.**
    - A lock resets the UI batch (`App.tsx:59`) without calling `launch_cancel`. The stuck timer (`usePlay.ts:84-87`) does the same for batches over 10 minutes.
    - Playing again makes the overlapping accounts fail with "Already launching." (`launcher.rs:330`).
    - The old batch's late `done` stays in `dones` forever.
  - **REACHABLE: batches the UI didn't start never settle.**
    - Sources: auto-relaunch (`relaunch.rs:396`), the API (`backend.rs:146`), reserve-then-join.
    - They fill `states` and `dones`, and the "one batch at a time" gate (`usePlay.ts:78`) doesn't see them.
  - **REACHABLE: silent cancel.** Cancel while starting (no id yet) does nothing.
  - **REACHABLE: side effects before the busy check.**
    - `saveLaunchSet` and `setDestination` run before the check (`usePlay.ts:76-78`, `Home.tsx:133`, tray `usePlay.ts:156`).
    - So a rejected Play still overwrites the Play bar and `lastLaunchSet`. Hero Play and OnlineNow have no busy guard of their own.
- **Mirror:** Rust `BATCHES`/`STATUS` reach React through events only. No status query exists to resync after a reload or lock.
**Fixed:** `usePlay.ts` is one store machine for every live batch (any source): progress from a batch the UI did not start is adopted onto the Play button (so the busy gate sees it, Cancel works) and every batch is pruned on its done; stuck timer removed (Rust now always emits `launch://done`, see H6); Cancel while starting is remembered and sent once the id arrives; busy check runs before `saveLaunchSet`/`setDestination` (new `play(ids, {dest})` used by hero, tray, Games, Servers join); tray Play does nothing while locked; new `launch_status` command (`launcher.rs`) resyncs live batches on mount (`useLaunchEvents`). Decision: one batch on the Play button at a time across sources, so a running auto-relaunch shows and briefly blocks Play.

### H6. Back-end batch and per-account launch progress: IMPLICIT (status strings) ✔
- **Where:** `launch/launcher.rs:37-38, 108-126, 149-234, 330-425, 455-464, 503-570`
- **Encoding:**
  - `BATCHES: HashMap<id, CancellationToken>`
  - an `IN_FLIGHT` set behind the `Flight` drop guard
  - the `STATUS` deque (max 200)
  - the `Own` drop guard, with a 30 s grace period
  - `Progress.state: &'static str`
- **States:**
  - batch: registered, running, cancelling, done (grace still pending)
  - account: queued → validating → launching → launched, failed or cancelled
- **Problems:**
  - **REACHABLE ✔: cancel can leave the account with no client.**
    - Cancel is not checked during install lookup, `write_fps` or `close_tracked` (`:382-400`); the first check comes after them (`:404`).
    - Result: the previous client is closed and the new launch is cancelled.
    - The comment at `:393` makes the close deliberate, but the "no client at all" outcome is not modelled.
  - **REACHABLE: lossy status.**
    - `STATUS` keeps only the last account's state per batch (`:230-232`).
    - So `/v1/launches/{id}` can say "launched" while an earlier account failed.
  - **REACHABLE: concurrent batches interleave.** No batch-level single-flight exists; batches from the UI, API and relaunch interleave, and only the per-account `IN_FLIGHT` guard protects them.
  - **REACHABLE: lock mid-batch.**
    - The remaining accounts fail one by one with LOCKED (`:345`).
    - The account already in flight still reaches `open_uri` (`:425`).
  - **THEORETICAL harm: two window pollers.** The detached `setup_window` keeps polling for up to 45 s (180 s plus 15 s of re-minimizing with `launch.startMinimized`) after `Flight` drops (`:455-464`), so a quick relaunch of the same account gets two pollers.
**Fixed:** `launcher.rs`: ticket fetched before any local side effect, cancel checked once at a commit point, then FPS write, close and open always finish together (cancel never leaves no client); `STATUS` keeps `accounts: {id: {state, message, gameName}}`; batches run one at a time via a `TURN` mutex (cancel skips the wait); vault `lock()` calls `cancel_all()` and `launch_one` re-checks the lock generation at the commit point and right before `open_uri`; `Own` drop guard removes the batch and always emits `launch://done`; `setup_window` (`platform/windows.rs`) exits when a newer poll for the same tracker starts.

### H7. Watcher per-client tracking (`Track`): IMPLICIT ✔
- **Where:**
  - `watcher/win.rs:134-153, 285-357, 405-458`
  - `logparse.rs:6-14`
  - `rules.rs:100-133`
  - `watcher/mod.rs:128-133`
- **States:** young, no log, foreign log (unsettled), settled with no evidence, joined, left, disconnected, close backoff, cooldown-blocked, gone.
- **Problems:**
  - **REACHABLE ✔: server state survives a log re-pick.**
    - The re-pick (`win.rs:309-316`) resets `log`, `log_pos`, `evidence` and `log_settled`, but not `srv`, `idle_since` or `last_event`.
    - Batch launches run 2.5 s apart. `claimed` is built in HashSet order (`:296-303`), so client B can take client A's log first.
    - State read from the wrong log then carries over:
      - a stale Joined suppresses NoServer
      - a stale Left can let NoServer (`rules.rs:114-120`) kill a client that is still loading slowly
  - **THEORETICAL-ish: endless log re-reads.** `srv == Unknown` keeps a client unsettled forever, so the log directory is re-read on every scan (`:285`).
  - **THEORETICAL: half-updated state and a small leak.** A panic caught mid-scan (`:511`) leaves a half-updated `Track`. `fires` keys for unmapped pids are never removed.
- **Why HIGH:** it decides which processes get killed.
**Fixed:** `watcher/win.rs`: `Track::repick` resets `srv`, `idle_since`, `last_event` with the log fields; logs are claimed oldest client first; re-picking stops once a client is past `LOG_AFTER_MS` (no endless directory reads); a caught scan panic clears all tracks (debug builds only: release builds abort on panic); an exited client drops its `pid:` cooldown key.

### H8. Vault lock lifecycle (Rust): IMPLICIT
- **Where:**
  - `lib.rs:37, 41`
  - `vault/mod.rs:118-149, 199-200`
  - `vault/store.rs:378`
  - `commands.rs:594-603, 686-955`
  - `shell.rs:156, 191-194`
- **States:**
  - no vault
  - locked; locked and damaged; locked with a corrupt payload (not modelled)
  - creating or unlocking (generation snapshotted, Argon2 running)
  - unlocked (OS or password)
  - portable-transient
  - locking
- **Problems:**
  - **REACHABLE: error shown for a vault that exists.**
    - Tray Lock during `vault_create`'s Argon2 run.
    - The file is already written (`commands.rs:712`), then `insert_if_current` fails with "The vault was locked."
  - **REACHABLE: vault converted while unlock reports failure.**
    - Tray Lock during a Portable unlock.
    - The file is re-wrapped to Password mode (`:790`) before the generation check (`:795`), so it is converted permanently while the unlock says it failed.
  - **THEORETICAL: two concurrent unlocks.** The second Vault is dropped (`:599`), but its `apply_remember` still runs (`:796`).
  - **REACHABLE: corrupt payload looks merely locked.**
    - A corrupt payload is reported as locked, not damaged.
    - Recovery is reached only by comparing the error string (`Unlock.tsx:133`).
- **Why HIGH:** seven entry points plus about 15 copies of the generation guard (X4).
- **Fixed:** lock wins (user decision): `vault::take_for_lock` bumps the generation again even when locked, so Lock cancels an in-flight unlock/create (phase 1's "bump only when taken" reverted; tests `lock_during_unlock_wins`, `lock_bumps_generation_even_when_locked` restored). `commands::insert_if_current` runs a `prep` step (portable re-wrap, Argon2 done before) only after the gen check under the slot mutex and returns the guard, so a cancelled Portable unlock never converts the file (test `lock_during_portable_unlock_leaves_file_portable`) and `apply_remember` runs under the guard; a concurrent second unlock drops its Vault and honours remember like the already-unlocked path. `vault_create` cancelled by Lock returns Ok (vault exists, locked; gate shows Unlock). `Unlock.tsx` shows no error for `VAULT_WAS_LOCKED`. Corrupt payload: `VAULT_DAMAGED` code + `hasCode` (phase 1) opens Recovery. Files: `vault/mod.rs`, `commands.rs`, `Unlock.tsx`. `vault::lock` now records the lock (bump + take) before closing session windows, so a sign-in poll in that gap gets `VAULT_WAS_LOCKED`, not "cancelled".

### H9. Vault mirror and app gate (React): IMPLICIT
- **Where:**
  - `App.tsx:41-81, 136, 141`
  - `vault/useVault.ts:7, 19`
  - `lib/ipc.ts:474-496`
  - `app/store.ts:14`
  - `lib/report.ts:14`
- **States:** loading, error, FirstRun, Unlock (auto-eligible or not), Shell. Locking also purges caches and launch state.
- **Problems:**
  - **REACHABLE: lock detection depends on error text.** Part of it matches English error text (X3). "The vault was locked." doesn't match, so no refetch follows.
  - **REACHABLE: early lock missed.**
    - The `vault://locked` listener attaches only after a dynamic import (`App.tsx:68-75`), so a lock before that point is missed.
    - The focus refetch partly covers this.
  - **No unlocked event.** Only the UI's own invalidations update the gate.
- **Fixed:** `vault_unlock`/`vault_create` emit `state_changed(["vault"])` (`commands.rs`); the `vault://locked` listener refetches status once it attaches, covering an early lock (`App.tsx`); error-text matching was replaced by codes in phase 1.
  - **REACHABLE: stale banner after unlock.** A `lastError` set while on the gate survives unlock and shows as a stale banner in the Shell.

### H10. Updater check → download → install → restart: IMPLICIT ✔
- **Where:** `shell.rs:24, 255-258, 338-366, 384-405`; `settings/SettingsPage.tsx:39-68`
- **States:** idle, available, checking (UI only), downloading/installing (exists only as the running future), paused (Windows), failed, restarting.
- **Problems:**
  - **REACHABLE ✔: a second concurrent install.**
    - `busy` is component state. Leave the section during the download, come back and click again: a second download and install starts. Rust has no in-flight guard (`shell.rs:386-405`).
    - The second `pause` returns `was_running = false`, so if the first install fails, its `resume` interleaves with the second.
  - **REACHABLE: stale Install button.**
    - A later check clears `PENDING` without emitting an event.
    - The UI still shows Install, which fails with "No update to install."
  - **REACHABLE, Windows only: automation restarted mid-install.** Between `pause` and exit the UI stays live, so the user can restart automation during the install.
- **Fixed:** Rust in-flight slot `Installing` (`shell.rs`): a second `update_install` returns `UPDATE_BUSY` (Settings keeps the button busy and says so), freed on failure, so no interleaved `pause`/`resume`; `automation_start` refuses with `UPDATE_BUSY` while an install runs (`commands.rs`). Stale Install button: fixed in phase 1 (`state_changed(["update"])` on every check).

---

## MED

### Launch / watcher

- **L1. Watcher run lifecycle (IMPLICIT).**
  - **Where:** `watcher/mod.rs:135-146, 188-233`; loop at `win.rs:497-539`.
  - **States:** stopped, running, stopping, restarting, plus a zombie state.
  - **Problems:**
    - THEORETICAL: the spawn-failure path (`win.rs:518-520`) leaves `running` true, after which `start()` does nothing.
    - REACHABLE, low impact: the old and new tasks share one `Notify` permit. On a quick stop→start, the old task sleeps a full `scanSec` and then flushes old window rects over newer ones.
    - `stop()` keeps `fires` and `last_fired`, so cooldown history survives a restart. This is not documented.
  - **Mirror:** the UI polls every `scanSec`, and a settings save does not invalidate `watcherStatus`. `watcher://action` is emitted (`win.rs:472`) but nothing listens for it.
  - **Fixed:** `watcher/mod.rs`: each run gets its own `Notify` (no shared permit); a run whose scan task dies calls `Watcher::ended(my)` (clears `running` unless a newer start/stop); kept cooldown history is documented on `stop()`; `reconcile` and every close emit `state_changed(["watcherStatus"])`; unused `watcher://action` event and `WatcherAction` type removed.

- **L2. Multi-instance mutex and cookie-lock guards (SEMI).**
  - **Where:** `lib.rs:33-35`; `commands.rs:115-166, 187`; `platform/windows.rs:413-462`.
  - **States:** off; held; wanted but blocked (Roblox owns the mutex); wanted but failed; wanted but unsupported. Every failure is stored as `None`.
  - **Design smell:** the `roblox_status` getter doubles as the retry loop.
  - **Problems:**
    - REACHABLE: `["roblox"]` (30 s `staleTime`) is not invalidated after a settings save, so `multiActive` and the banner go stale.
    - REACHABLE: `MultiBanner`'s `gone` flag never resets (`PlayBar.tsx:565`).
    - REACHABLE: turning multi off and back on while clients run stays blocked until every client closes, and the UI never explains why.
  - **Fixed:** `["roblox"]` was already invalidated by `settings_set`/`multi_set` (`state_changed`), so the banner now appears and explains the block; `MultiBanner` (`PlayBar.tsx`) mounts only while blocked, so its dismiss resets. The getter-as-retry design smell is kept.
  - **Later:** `roblox_running` returns `Option<bool>`; a failed process snapshot is unknown, never "nothing runs". The mutex stays held, auto-relaunch marks nobody exited, login start and watcher reconnect launch nothing, a hand-over counts every client as still running, the friend account list answers 503 "try again", and settings sidecars are restored only on a sure "no client runs". With multi-instance unavailable `start_batch` refuses more than one account (`ONE_CLIENT`).

### Automation

- **A1. WebSocket connection lifecycle (IMPLICIT).**
  - **Where:** `automation/ws.rs:72-78, 153-161, 183-188, 278-319, 580-616`.
  - **States:** upgrading, rejected, registered, kicked-draining, drained, closed.
  - **Problems:**
    - REACHABLE, under a broadcast storm: on a full queue the connection is removed (`:183-188`), but the socket stays open and flushes up to 256 frames. Inbound frames are dropped (`:305`). The engine sees a fall and arms a relaunch while the script and game are still alive.
    - REACHABLE: a `hello` re-key (`:313-319`) flips the old account to disconnected, which can arm a relaunch.
  - **Fixed:** `ws.rs` `serve` closes as soon as its queue is closed by the hub (no 256-frame flush; 1013 at once, so the script reconnects and the rise cancels the pending); a re-key that leaves the old name with no client records it, `relaunch_view` hands it over in the same hub lock, and the loop calls `Engine::quiet` on it before the tick (`relaunch.rs`).

- **A2. Token lifecycle (IMPLICIT).**
  - **Where:** `automation/mod.rs:129, 225, 298-382`.
  - **Encoding:** one `token_loaded` bool stands in for six states: ephemeral, loaded, saved, memory-only, reset, regenerated.
  - **Problems:**
    - `["autoToken"]` is cached forever. Enabling automation swaps in the saved token (`mod.rs:371-373`) without invalidating the cache, so the UI can show a dead token. Not checked whether the row is visible while automation is off.
    - Regenerate has no pending guard; a double click is harmless.
  - **Fixed:** `commands.rs` `token_ready` emits `state_changed(["autoToken"])` when `ensure_token` swapped the token (compared before/after, so the refetch cannot loop). Regenerate double click: no change (Rust `token_lock` serializes; the last answer wins, as noted).

- **A3. Relaunch status mirrored in `["snap"]` (IMPLICIT, spans both layers).**
  - **Where:** `commands.rs:401-424`, `useAutomation.ts:12`.
  - **Problems:**
    - REACHABLE: `reset()` emits nothing, and `disconnect_all` marks only connected accounts as changed (`ws.rs:292-299`). A disconnected account keeps showing "relaunch at HH:MM" after Stop or a regenerate.
    - REACHABLE: the cooldown warning persists after re-enable, because the refetch beats the tick that clears it.
    - The "Launching" label is right only because of the 100 ms coalescing delay (`ws.rs:229`).
  - **Fixed:** `ws.rs` `disconnect_all` and the loop's resets mark every account changed (`Hub::touch_all`); `Fin::drop` touches the account after clearing in-flight; a resume touches it; `lib.rs` sends `state_changed(["snap"])` after every hub change batch and `vault::lock` sends it on lock; `useAutomation.ts` drops its page-mounted `automation://changed` listeners (app-wide `useStateSync` instead).

### Accounts

- **C1. Remove → undo window (IMPLICIT).**
  - **Where:** `accounts/mod.rs:446, 598, 619`; `accounts/cmds.rs:266-314`; `useAccounts.ts:92-93`; `Toasts.tsx:22, 38-49`.
  - **Encoding:** two independent clocks. Rust uses a fire-and-forget 8 s sleep; the toast counts 8 s but pauses on hover.
  - **Problems:**
    - REACHABLE: Undo after Rust's window has expired fails with "Undo window has passed."
    - REACHABLE, needs a save failure: if undo's save fails after the timer fired, the re-insert has no timer, so removed secrets stay in memory until the next lock. A lock between the take and the re-insert puts items back into the map after it was cleared.
    - REACHABLE: remove an account, re-add the same user within 8 s, then Undo. `restore` dedupes by id only, so two accounts share one `userId`.
  - **Fixed:** Rust holds removed items 120 s (toast stays the only practical clock); `hold()` re-inserts under the map mutex only if no lock since, with a fresh timer; `restore` skips an id or userId re-added meanwhile (`accounts/cmds.rs`, `accounts/mod.rs`, contract).

- **C2. `SessionState` lifecycle (SEMI).**
  - **Where:** `accounts/mod.rs:24-31, 102, 139-160, 288, 301, 585-591`; `sync.rs:68, 209, 238-243, 276-278`. See also X5.
  - **Problems:**
    - REACHABLE: an offline validation downgrades expired or ok to unknown. The Inspector then shows Validate instead of "Sign in again", and presence loses its cookie.
    - THEORETICAL: an ok account with an empty cookie stays ok forever.
    - Diverges from the contract: the contract says `userId` is null only while pending.
    - REACHABLE: `last_validated_at` is saved only when some other field is dirty, and a failed save leaves memory and disk out of sync.
  - **Fixed:** `apply_checks` ignores `Unknown` (offline) results, marks an `Ok` account with no cookie `Expired`, counts `lastValidatedAt` as a change; a failed save rolls memory back (`accounts/sync.rs`). userId: no writer makes it null outside pending (only hand-damaged rows), so no code change.

- **C3. Optimistic `["accounts"]` cache vs. event-driven refetch (IMPLICIT).**
  - **Where:** `useAccounts.ts:33, 63-81, 130-138`.
  - **Encoding:** no in-flight marker and no `cancelQueries`.
  - **Problems:**
    - REACHABLE: a presence-triggered refetch lands after the optimistic write and reverts a drag or a patch.
    - REACHABLE: `updateMany` sends N IPC calls, each emitting `changed`. The UI flickers through half-applied states, and a partial failure keeps the earlier saves.
    - REACHABLE: moving an account to a group doesn't patch `order`. Rust moves it to the end (`mod.rs:413-415`), so it jumps on refetch.
  - **Fixed:** `saving()` counts saves in flight; `accounts://changed` refetches wait for the last one and a refetch already running is cancelled by an optimistic write; `accounts_update` takes `ids` (one atomic save, one event); the optimistic group move patches `order` like Rust (`movedOrder`) (`useAccounts.ts`, `accounts/cmds.rs`, `ipc.ts`, `mock.ts`, contract).

- **C4. Add sheet bulk paste (IMPLICIT).**
  - **Where:** `AddSheet.tsx:172-199`; store `add.{open, tab, prefill, prefillFor, nonce}`.
  - **Problems (all REACHABLE):**
    - The textarea stays editable while busy, so a success wipes anything typed meanwhile (`:188-193`).
    - An error doesn't clear the old `results`.
    - Line numbers drift after the successful lines are removed.
    - Dropping text onto the list mid-submit bumps `nonce`; the remount loses the in-flight result.
  - **Fixed:** textarea read-only while busy; an error clears old results; rejected lines renumbered to the new text, done rows unnumbered (`afterAdd`); no `key={nonce}` remount: a new nonce resets the form only when idle, text dropped mid-submit is appended after the leftovers (`AddSheet.tsx`).

- **C5. Validation single-flight (SEMI).**
  - **Where:** `accounts/mod.rs:607`, `sync.rs:162-200`, `cmds.rs:587`.
  - **Problems:**
    - REACHABLE: TOCTOU. A plain load pre-check followed by a later swap means the UI can toast "validating" for a pass that never runs.
    - REACHABLE: a click during a loop pass is ignored with no feedback.
  - **Fixed:** `sync::Flight::claim` takes the slot before spawning, the pass owns it (loop, `accounts_validate`, import all claim); a busy click toasts "A session check is already running." (`accounts/sync.rs`, `cmds.rs`, `import/cmds.rs`, `useAccounts.ts`).

### Vault / import

- **V1. Auto-lock timing (IMPLICIT).**
  - **Where:** `lib.rs:184`, `vault/mod.rs:169-200`, `App.tsx:21, 85-95`, `commands.rs:558`.
  - **Encoding:** wall-clock time, with the rules split over three places.
  - **Problems (all REACHABLE):**
    - Setting the clock back suppresses the idle lock.
    - A forward jump of more than 30 s with `lockOnSleep` on causes a spurious lock.
    - Only the main webview counts as activity; the sign-in windows don't.
  - **Fixed:** activity and the idle check use `vault::mono_now()` (monotonic, `Instant`), so clock changes no longer stretch or cut idle; on Windows sleep is detected on the monotonic gap (QPC counts sleep, ignores clock jumps); mac/linux keep the wall-clock gap (ponytail note). Focusing any app window (sign-in/site windows too) counts as activity (`lib.rs` `WindowEvent::Focused`). Files: `vault/mod.rs`, `commands.rs`, `lib.rs`.

- **V2. Remember-device cache (IMPLICIT).**
  - **Where:** `vault/mod.rs:72-103, 141-142`; `commands.rs:739-741, 775, 840, 949`.
  - **Problems:**
    - ✔ A crash between `apply_rewrap` and `cache_remove` (`commands.rs:839-840`) leaves the old cache in place. The data key never changes, so that cache still opens the vault for up to 7 days without the new password. The window is tiny.
    - A valid prefix with a bad body makes `rememberActive` true, and the silent auto-unlock then fails.
    - An expired cache file is purged only on lock.
  - **Fixed:** `change_password_impl` removes the cache before the re-wrap; a passwordless unlock whose cache can't be read (expired, bad body) deletes it, so `rememberActive` stops lying and expired files go at startup too (`commands.rs`, test in `stale_cache_falls_back_and_is_removed`).

- **V3. Atomic write, create and backup sequence (IMPLICIT).**
  - **Where:** `vault/store.rs:170-222, 383-405`; `commands.rs:700, 941-948`.
  - **Problems:**
    - ✔ A crash between the `create_new` placeholder and the rename leaves a 0-byte vault. Status then reports "exists, damaged", there are no backups (`Backup::Skip`), and `vault_create` refuses with "A vault already exists." The user is stranded. The window lasts only for `write_atomic`.
    - REACHABLE after any crash: orphaned `*.tmp` files are never swept. For `vault_export_plain`, the orphan is plaintext JSON in the export folder.
    - THEORETICAL: the damaged-copy name has 1-second resolution, and `fs::copy` overwrites an existing file.
  - **Fixed:** `Vault::exists` ignores a 0-byte file and `Vault::create` replaces it (first run works again); `write_atomic` sweeps `<name>.<pid>-<n>.tmp` leftovers of other pids before writing (exports included); the damaged copy gets a `-1`, `-2`... suffix instead of overwriting (`vault/store.rs`, `commands.rs`, test `crashed_create_and_tmp_leftovers_recover`).

- **V4. Import preview slot (IMPLICIT).**
  - **Where:** `accounts/mod.rs:609, 623`; `import/cmds.rs:30-39, 243-287, 533-599`.
  - **Problems:**
    - REACHABLE: there is a single slot, so a second preview kills the first id. A commit on the old id then says "preview has expired".
    - REACHABLE: commit is not atomic. It replaces the vault (`:541`), then saves settings (`:547`), then library.json (`:586-587`). A later failure becomes only a note and the slot is cleared (`:599`), so nothing can be retried without re-entering the password.
    - Settings and library file I/O run while the vault mutex is held.
  - **Fixed:** settings and library.json writes run after the vault and preview locks are released; on a settings/games failure the preview is kept and `CommitOut.retry` makes Results offer "Try again" (accounts come back updated/skipped, games dedupe); the slot is cleared only on full success. A second preview overwriting the first is prevented by the app-wide single-flight in V5. Files: `import/cmds.rs`, `ImportSheet.tsx`, `ipc.ts`, `mock.ts`.

- **V5. Import sheet flow (IMPLICIT).**
  - **Where:** `ImportSheet.tsx:53-129, 185-197, 424-427`.
  - **Encoding:** nine variables, including `busy: "look" | "read" | "import" | null`.
  - **Problems:**
    - REACHABLE: the path field stays editable during "read". When the read resolves, the old folder's preview shows under the new path.
    - REACHABLE: a "look" started during a "read": the read's `finally` clears `busy` mid-detect, breaking the one-detection-at-a-time guard.
    - Preview expiry is detected by matching a substring of the Rust message.
    - The UI says "gave up" at 300 s, before the Rust retry loop actually ends.
  - **Fixed:** module-level `inFlight` guard in `ImportSheet.tsx` (one detect/preview/commit at a time app-wide, survives remount); folder input, Browse, Look and file checkboxes are disabled while any step runs; expiry uses `hasCode` (phase 1); "gave up" now at 360 s (Rust's 300 s slot wait plus the check).

### Discover / tools / HTTP

- **D1. Server-list stream (SEMI in Rust, IMPLICIT in React).**
  - **Where:** `discover/mod.rs:171-247, 373`; `lib.rs:192`; `vault/mod.rs:138`; `serverModel.ts:84-191`; `Servers.tsx:68, 81, 114, 262-272`.
  - **Problems:**
    - THEORETICAL today: cancel emits no terminal event, so the front end would sit at `streaming: true` forever. Only the lock unmounting the Shell hides this (X1).
    - REACHABLE, cosmetic: a stopped stream looks the same as a finished one, so a partial list is announced as "loaded".
    - REACHABLE: rows aren't tagged with the kind they were fetched as, so toggling public/private shows the old rows.
    - REACHABLE: a 401 on a private stream never calls `tools::expire`, so the account stays `ok` (X5).
  - **Fixed:** a cancelled stream ends with one empty `done` page marked `stopped` (`discover/mod.rs` `run_pages`); `useServerStream` keeps `stopped` (also on Stop and the row cap) and the live region says "Stopped at N servers" instead of "loaded" (`serverModel.ts`, `Servers.tsx`); switching public/private refetches (`pickKind`). 401: fixed in phase 1.

- **D2. Find-player scan (IMPLICIT).**
  - **Where:** `serverModel.ts:196-240`; `discover/mod.rs:259-315, 396-443`.
  - **Problems:**
    - THEORETICAL: the bools allow running and done at the same time.
    - REACHABLE: a 429 mid-scan retries for up to about 4 minutes with no per-scan indicator. Five 429s end the scan as an error.
    - Same cancel-on-lock gap as D1.
  - **Fixed:** one `phase: idle | running | done` replaces the bools (`serverModel.ts` `useFindPlayer`); a cancelled scan (also during the headshot lookup) emits a `stopped` terminal event and goes back to idle (`discover/mod.rs` `find_stopped`); while Roblox rate-limits, the status says "Roblox asked us to slow down, waiting…" (`Servers.tsx`, `useRateLimited`). The 5-try limit stays (client policy, error shown).

- **D3. Region loading (IMPLICIT).**
  - **Where:** `Servers.tsx:73-74, 112, 167-179, 520-526`; `discover/mod.rs:534`.
  - **Problems:**
    - REACHABLE: the context-menu load isn't gated by `regionsBusy`. The first `finally` clears busy while the second load is still in flight, and the duplicates spend the gamejoin budget.
    - REACHABLE: a refresh during a load lets the old response merge into the freshly cleared map.
    - REACHABLE: a geo-template change leaves the front end's regions stale.
  - **Fixed:** `loadRegions` (only entry point) has a ref guard, and the menu item is disabled while busy; an epoch bumped on every clear drops responses from before a refresh; a `geo` settings change clears the map (`Servers.tsx`).

- **D4. HTTP client CSRF refresh and 429 backoff (IMPLICIT).**
  - **Where:** `roblox/client.rs:65-66, 250-313`; `shared.tsx:75-87`.
  - **Problems:**
    - REACHABLE: there is no shared per-host cooldown, so N callers back off independently and hit the host again on their own timers.
    - REACHABLE: the front end's `until` is overwritten by any host's event, and giving up emits nothing.
    - THEORETICAL: `csrf_retried` is never reset across 429 loops.
    - The CSRF map is never evicted.
  - **Fixed:** shared per-host `cooldown` set by every 429 (only pushed out) and waited out at the top of each try (`client.rs` `back_off`/`wait_cooldown`); `csrf_retried` resets on a 429; a 401 drops that cookie's CSRF token. React keeps the longest announced wait across hosts (`shared.tsx` `useRateLimited`); give-up needs no event since it comes right after the last announced wait ends.

- **D5. Tool calls vs. lock generation and session (SEMI).**
  - **Where:** `tools/mod.rs:68-79, 122-131, 150-250, 402-442`.
  - **Problems:**
    - REACHABLE: lock and reopen mid-batch. Display-name saves use the old generation, so every ok row reports "not saved here".
    - REACHABLE: `expire` emits `changed`, and the `ready` gate then unmounts the pane mid-flow, losing its status.
  - **Fixed:** display names save per row under the generation that row's cookie was read in (`tools/mod.rs` `save_display_name`; `run_batch` rows can now be ok with a note); a pane once shown for a selection stays mounted when a session expires (tab switches stay blocked until ready) (`ToolsSheet.tsx` `shownFor`).

- **D6. Favorites optimistic updates and undo (IMPLICIT).**
  - **Where:** `Favorites.tsx:51-96`; `launch/recents.rs:18, 69-83`; `discover/mod.rs:811-820`.
  - **Problems:**
    - REACHABLE: two quick drags run two blocking tasks. The lock serializes them but doesn't order them, so the disk can keep the older order.
    - REACHABLE: remove, then reorder, then undo replays the stale `before` snapshot.
    - REACHABLE: a background refetch briefly brings back a removed row.
  - **Fixed:** every library call takes a fair FIFO `tokio::Mutex` turn before its blocking task, so edits hit disk in call order (`discover/mod.rs` `on_library`); undo re-inserts at the old index into today's order (`Favorites.tsx` `undoOrder`); optimistic edits cancel in-flight favorites queries first.

- **D7. Reserve-then-join (IMPLICIT).**
  - **Where:** `Servers.tsx:153-160`; `discover/mod.rs:563-591`.
  - **Problems:**
    - REACHABLE: there's no in-flight flag while reserving; the guard reads `useLaunch.batch`, which stays unset until the launch starts. A double click starts two reserve loops and two `launch_start` calls.
    - REACHABLE: the gamejoin `status` is only logged (`:581`), so a full or not-ready server still counts as reserved.
    - The returned BatchId is ignored (see H5).
  - **Fixed:** Rust `RESERVING` guard: a second `servers_reserve_join` during the reserve loop returns `RESERVE_BUSY` (shows "A launch is already running."); when every gamejoin answer is 5/6 (ended/full) it stops with "That server is full or has closed." instead of launching (`discover/mod.rs` `closed_server`). BatchId: H5.

- **D8. Tools action and batch status (SEMI).**
  - **Where:** `useTool.tsx:28, 92-120`; `panesMore.tsx:240-333`.
  - **Problems:**
    - REACHABLE: block and unblock-all have separate busy flags, so both can run on the same account at once.
    - `prog` is stale after a run.
  - **Fixed:** one `busy = b.busy || x.busy` gates block, unblock, unblock-all and Enter (`panesMore.tsx` `Blocks`); `prog` resets when a run ends (`useTool.tsx`).

### App core

- **P1. Settings optimistic save (IMPLICIT) ✔.**
  - **Where:** `useSettings.ts:25-37`, `commands.rs:47-88`, `SettingsPage.tsx:208`.
  - **Encoding:** "last to finish wins".
  - **Problems:**
    - REACHABLE, transient: a slow save A (enabling automation awaits the keyring and reconcile, `commands.rs:56-60`) that finishes after a fast save B writes A's older snapshot (`useSettings.ts:31`). B's value flips back until A's `onSettled` refetch lands.
    - `client_settings_set` writes the cache outside the mutation gate.
    - Rust-side writers (`shell.rs:284, 302`; `commands.rs:220-224`) emit no `settings://changed`.
    - After a save that succeeded, the error reads "Settings saved, but…".
  - **Fixed:** `settings_set` returns the settings as they are after its slow side effects, not its own snapshot (`commands.rs`); `client_settings_set` writes the cache only when no `KEY_SET` save is in flight (`SettingsPage.tsx`); `client_settings_set`, `multi_set`, `cookie_lock_set` emit `state_changed(["settings"])` (shell writers already did), and `useStateSync.ts` skips `["settings"]` while a save is in flight (its settle refetch covers it). "Settings saved, but…" kept on purpose: it is a true partial failure, and the error path refetches settings (`useSettings.ts` `onSettled`).

- **P2. Startup ordering and UI readiness (IMPLICIT).**
  - **Where:** `lib.rs:295-389`.
  - **Problems:**
    - REACHABLE: the automation notice is emitted once, after a fixed 5 s sleep (`:373-374`). Its listener lives in the Shell, so the notice is always dropped for password-mode vaults (Unlock screen showing) or on a slow webview.
    - THEORETICAL: `lib.rs:383` says the guards are taken before anything can start Roblox, but the relaunch engine (`:332`) and the automation start (`:356-381`) spawn before `sync_guards` (`:386`). The keyring await happens to prevent this in practice.
  - **Fixed:** no sleep; Rust keeps the latest notice until the UI takes it (`automation_notice_take`, `commands.rs`), which `useSecretToast.ts` does on mount and on every `automation://notice`; `sync_guards` now runs first in `setup` (`lib.rs`).

- **P3. Close-to-tray hint (IMPLICIT).**
  - **Where:** `shell.rs:23, 147-148, 300-305`; `App.tsx:45`.
  - **Problems:**
    - REACHABLE: the "seen" flag is persisted before the hint is shown. Quitting or rebooting before the window reopens means the hint never appears.
    - An emit before the listener attaches is lost.
  - **Fixed:** explicit `HINT` none → due → shown (`shell.rs`): close only marks it due; `show_main` emits `tray://hint` and only then persists `trayToastSeen`; shown once per run even if the flag can't be saved. The hint only fires on a re-show of a window that was already loaded, so the App-level listener is attached.

- **P4. Settings read-only mode (IMPLICIT).**
  - **Where:** `lib.rs:29`, `settings/mod.rs:222-243`.
  - **Problems (all REACHABLE):**
    - The UI doesn't know about it, so every toggle flips, errors, then reverts.
    - `read_autostart` can't persist, so the UI shows launch-on-login off while the OS has it on.
    - The tray hint is never set.
  - **Fixed:** read-only `persist` applies the patch in memory for the session and never writes the file (`commands.rs`), so toggles, the autostart mirror and the tray hint work; `app_info.settingsReadOnly` drives a note on the Settings page (`SettingsPage.tsx`, `settings.readOnly`).

- **P5. Command palette and modal flags (IMPLICIT).**
  - **Where:** `store.ts:22-28`; `Palette.tsx:50-151`; `TitleBar.tsx:53`.
  - **Problems:**
    - REACHABLE: running a command with Enter, or closing with Ctrl+K, doesn't reset `q`/`active`, so the stale query reappears on the next open.
    - The four open flags (`add.open`, `importOpen`, `toolsOpen`, `paletteOpen`) are independent bools; keeping them exclusive relies on DOM checks.
  - **Fixed:** `Palette.tsx` resets `q`/`active` on every close (effect on `open`); `store.ts` subscribes once and closes the other modals whenever one opens, whoever opened it.

- **P6. Two toast systems (IMPLICIT).**
  - **Where:** legacy single slot at `store.ts:15-16, 52`, `Toast.tsx:20-24`; queue at `Toasts.tsx:16-50`.
  - **Problems:**
    - REACHABLE: the two overlap on screen, and the legacy slot overwrites its previous message.
    - THEORETICAL: action toasts in the queue are never evicted, so they can grow without bound.
  - **Fixed:** legacy slot removed (`Toast.tsx` deleted, `store.ts` `toast`/`setToast` gone); `useSecretToast.ts` uses the queue; `Toasts.tsx` caps plain and action toasts at 3 each.

---

## LOW

### Launch / watcher
- `setup_window` polls every 100 ms for the first 10 s, then every 500 ms, detached, until the window shows or 45 s pass (180 s with `launch.startMinimized`); a minimized launch then re-minimizes it every 200 ms for another 15 s (`REMINIMIZE_FOR`). A newer poll for the same tracker cancels it (`platform/windows.rs` `setup_window`, `poll_interval`).
- `close_verified` runs WM_CLOSE, waits 1 s, terminates, waits 2 s (`windows.rs:707-726`). Sequential and correct.
- `hadFocus` plus the `running` effect refocuses Play after a batch settles; `hadFocus` goes stale across entry points (`PlayBar.tsx:510-513, 608-614`).
- `Slot` animation `flight`/`landed` timers (`PlayBar.tsx:319-350`).
- `Chip` debounce: `typing = q !== input` plus the query state (`PlayBar.tsx:31-93`).
- `client_settings_set` writes the file copy, then the setting; a persist failure leaves the copy behind (`commands.rs:222-224`).
- **Fixed (LOW launch/watcher):** `setup_window` superseded by a newer poll (`windows.rs`); mutex owner thread is (re)started on demand with global guard ids (`owner_send`, `windows.rs`); `hadFocus` moved into the launch store (`focus`, set per play) and refocus only on a running-to-idle change (`PlayBar.tsx`); `Slot` shows when motion turns off mid-flight (`shown = landed || !fly`); `client_settings_set` writes `client-settings.json.new`, saves the setting, then renames (`commands.rs`). No change: `close_verified` (correct), `Chip` debounce (no bug found).

### Automation
- Relaunch batch-follow treats an evicted `batch_status` entry as a failure and re-arms (`relaunch.rs:401-421`). ✔ `STATUS_MAX = 200` (`launcher.rs:203`) makes eviction within 600 s unlikely.
  **Fixed:** a lost status (evicted or past `FOLLOW_MAX`) no longer re-arms (the batch may still launch; a re-arm could launch twice); console note "Relaunch status lost; not retried" (`relaunch.rs`).
- Own-launch grace map: `own_busy` mutates on read, and a count leaks if `Own` is never created (`automation/mod.rs:222, 272-283`; `launcher.rs:505-539`).
  **Fixed:** `own_busy` is read-only, `own_begin` prunes ended graces (`automation/mod.rs`); `Own` is built before the spawn and moved in (`launcher.rs`).
- Loop poll state `had`/`n`/`seen`/`global_since`/`last_min`: exits seen while not serving are dropped, which is intended (`relaunch.rs:319-329, 482-496, 592-621`).
  **No change:** intended.
- Hub change coalescing with `dirty` + `Notify` + 100 ms (`ws.rs:82-84, 218-232`). Fine.
  **No change:** fine.
- `AppBackend` `OnceLock` host: starting → ready, answering 503 until set (`backend.rs:19-39`).
  **No change:** one-way, set once in `setup` before the servers can bind; 503 is the right answer before.
- Relaunch delay `draft: string | null`; the toggles are fire-and-forget with no in-flight guard (`AutomationPage.tsx:110-130`).
  **Fixed:** the toggle/delay update the `["snap"]` cache optimistically, so a quick second click toggles from the new value; Rust gets the calls in order and the refetch (also on error) settles it (`AutomationPage.tsx`).
- Controls `Field` draft plus a `last` ref for dedupe, remounted on the server value (`Controls.tsx:22-35, 85`).
  **No change:** works as described.
- The `useCopy` 1.5 s timer isn't cleared on unmount (`TokenSettings.tsx:12-20`).
  **Fixed:** timer lives in an effect keyed on the copy time (cleared on unmount, restarted by a repeat click) (`TokenSettings.tsx`).
- ConsoleView `clearedAt` is lost on account switch; `stick` autoscroll ref (`AutomationPage.tsx:218-275, 367`).
  **Fixed:** per-account clear marks in a module `Map` (survive switch and page visits) (`AutomationPage.tsx`); `stick` per mount is fine.
- `picked` silently falls back to `snap[0]` when its account is evicted (`AutomationPage.tsx:344-345`).
  **No change:** showing the first account is the least surprising view; the pick comes back if that account reconnects.

### Accounts
- Reauth window: a Rust deadline `reauth_until` plus a React singleton promise; a second request cancels the first (`mod.rs:600`, `cmds.rs:490-498`, `Reauth.tsx:12-19`).
- Clipboard clear: `clip_seq` epoch plus a 30 s timer; a lock doesn't clear a copied secret immediately (`cmds.rs:430-457`).
- Headshot fetch has no dedupe, so `kick_headshots` races the loop on the same `.tmp` file (`sync.rs:180, 367-370`).
- Presence map: entries the API stops returning stay forever (`mod.rs:596`, `sync.rs:36-51, 103-105`).
- Drop-zone `dropping`: empty `text/plain` data returns before the reset, so the outline stays stuck. REACHABLE (`AccountsScreen.tsx:148, 405-406`).
- Inspector drafts (`draft` null means not editing; GroupPicker `adding`/`name`). Esc revert relies on the rAF blur order (`Inspector.tsx:192-261, 368`).
- Selection `sel {ids, anchor, cursor}` is written from many sites, plus a `focusReq` handshake (`AccountMenu.tsx:71, 195`; `useAccounts.ts:92`; `AccountsScreen.tsx:178`; `Inspector.tsx:195-261`).
- **Fixed (LOW Accounts):** reauth: a second request shares the open sheet's answer (`Reauth.tsx`); lock clears a copied secret still on the clipboard (`clip_secret`, `clear_secret_clip`, `Runtime::on_lock`); headshots write a unique `.tmp` and remove it on failure (`sync.rs`); presence drops entries for removed accounts or users Roblox stops returning (`sync.rs`); drop-zone resets `dropping` before the empty-data return (`AccountsScreen.tsx`); Inspector Esc uses a sync `cancelled` ref, GroupPicker `done` runs once (`Inspector.tsx`). Selection/`focusReq`: no failing path found, unchanged.
- Group drag `dragGroup` (`AccountsScreen.tsx:150, 297, 446-450`). Fine.

### Vault / import
- Unlock screen `pw`/`remember`/`error`/`pending` plus the `ran` ref: after a successful Recovery restore, the stale "damaged" error stays until the next submit. REACHABLE (`Unlock.tsx:26-67, 133`).
  **Fixed:** `Recovery` `onRestored` clears Unlock's error and damaged flag (`Unlock.tsx`, `Recovery.tsx`).
- VaultSection `useAction`: the query invalidation unmounts the mode form before `setOk`, so "toOsDone"/"toPwDone" never show. REACHABLE. Backups `ok` never clears (`VaultSection.tsx:50-67, 107, 167-186`).
  **Fixed:** mode switches toast their success; Backups `ok` clears when a new restore is asked (`VaultSection.tsx`).
- Re-wrap optimistic concurrency (SEMI): a `Keys` snapshot, an equality check and a rollback (`store.rs:477-490`). Handled.
- OS wrap lazy init (`Mutex<Option>`) plus the mint/refuse ladder; cached forever once set (`lib.rs:45-59`, `oswrap.rs:119-159`).
  **Not fixed:** no failure listed; caching the OS wrap for the process lifetime is intended.
- FirstRun `serverErr` carries over a mode switch (`FirstRun.tsx`).
  **Fixed:** cleared on mode change (`FirstRun.tsx`).
- Recovery `open`/`pw`/`pending`/`err` plus a backups query gated on `open`. Fine.

### Discover / tools / HTTP
- Outfit wear runs 4 sequential POSTs with no rollback, so a failure leaves the avatar half-applied. The panel can belong to an old account (`roblox/api/avatar.rs:53-102`, `Outfits.tsx:38-59`).
  - **Fixed:** no rollback (ponytail note in `avatar.rs`); a failure after an applied step now says "Only part of the outfit was applied…" (401 stays a 401); the missing-items panel is tagged with its account and hidden for any other selection (`Outfits.tsx`).
- Site-as-account window: build, set cookie, navigate, check `still_unlocked`, destroy. A lock closes it twice (`tools/site.rs:40-86`).
  - **Fixed:** the post-setup lock check skips `destroy` when the lock already closed the window (`site.rs`).
- `refreshReq` counter plus a `handled` ref handshake; it works only because Servers is force-mounted (`shared.tsx:20, 39-40`; `Servers.tsx:119-126`; `Discover.tsx:58`).
  - **Fixed:** `refreshPending` flag that Servers clears when it refreshes, now or on mount (`shared.tsx`, `Servers.tsx`).
- Confirm-then-act `ask` isn't reset on error (Sessions, unblock-all) (`panesMore.tsx:157-191, 236, 297-331`).
  - **Fixed:** the confirm closes after the run on any outcome; the result shows in Status (`panesMore.tsx`).
- PinChip countdown: query plus interval plus `now`; never refetched (`ToolsSheet.tsx:207-230`).
  - **Fixed:** refetched via `state_changed(["pin"])` on every `PIN_REQUIRED` (see H1).
- Games search debounce plus an infinite query keyed by the search (`shared.tsx:65-72`, `Games.tsx:38-50`). Safe.
- Games favorite toggle: per-tile `favBusy`, with the direction decided from the cached `fav` (`Games.tsx:260-272`).
  - **Fixed:** no code change needed: `favorites_add` dedupes, `favorites_remove` is idempotent, the list refetches after, and library calls now run in order (D6).
- Thumbnail `state == "Completed"` string compares; `target_headshot` retries once (`discover/mod.rs:402`, `tools/mod.rs:910`).
  - **Fixed:** no problem listed; left as is.
- RateLimiter token buckets: per-cookie keys are never evicted, and the host token is spent while waiting on the account bucket (`ratelimit.rs:7-28`, `client.rs:253-257`).
  - **Fixed:** buckets idle 60 s (full again) are dropped once over 256 keys (`ratelimit.rs`); the account bucket is acquired before the host one (`client.rs`).
- Universe `bad` plus `placeId`: the error shows next to the previous results. Recents `clearing` (`Universe.tsx:18-19`, `Favorites.tsx:247`).
  - **Fixed:** a bad id clears `placeId`, so old results go (`Universe.tsx`); Recents `clearing` needs no change.

### App core
- NumberCtl draft plus a 400 ms debounce: unmount clears the timer, so a change followed by a lock within 400 ms loses the save (`controls.tsx:90-126`).
  **Superseded:** NumberCtl has no timer now. Every Settings edit goes to the unsaved-edits store `useDraft` (`settings/draft.ts`: `edits`, `setAll`, `clear`, `dropSent`) with no IPC; the Save and Discard bar (`SettingsPage.tsx`) owns the rest. Save sends the edits through `KEY_SET` and `dropSent` removes only what was sent; Discard clears; leaving the page keeps the edits (module store) unsaved until Save or Discard. P1's optimistic `KEY_SET` path still applies to that Save and to saves that bypass the draft (Shell toggles such as privacy mode).
- `settings_set` runs its side effects outside the settings lock; concurrent saves reconcile in any order but converge (`commands.rs:54-60`).
  **Fixed:** each side effect reads the current settings (so the last one converges), and `settings_set` now returns the settings read after them rather than its own snapshot (`commands.rs`).
- Tray existence decides close vs. quit: a failed tray `create` silently means quit on close (`shell.rs:261-296, 325-332`).
  **Fixed:** `on_close` retries the tray build first (`sync_tray`), and logs a warning when the tray is enabled but missing, so close quits (`shell.rs`).
- The `automation` route has no guard and survives `automation.enabled = false` (`store.ts:154, 203`; `Sidebar.tsx:22`).
  **Fixed:** the Shell doesn't render the page while automation is off and goes to Accounts (`Shell.tsx`). While a remote session window is open `go()` does nothing, so the route stays `automation` (page not rendered) until the session closes.
- `lastError` is a single global slot where the last write wins (`store.ts:14`, `report.ts:14`, `Shell.tsx:89-101`).
  **Fixed:** later errors queue behind the shown one in `moreErrors` (duplicates dropped, at most 5); each dismiss shows the next (`store.ts`).

---

## Coverage notes

- **Only grepped (no state found):** `launch/dest.rs`, `recents.rs`, `uri.rs`, `clientsettings.rs`.
- **Not opened:** `watcher/rules.json`.
- **Skimmed:** `roblox/api/*` wrappers other than `avatar.rs`, `account.rs` and `gamejoin.rs`.
- **Parsing only, skimmed:** `import/mod.rs` lines 200-434.
- **Excluded:** tests and `src/lib/mock*.ts`.
