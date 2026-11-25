import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, waitFor } from "@testing-library/react";
import { emptySel } from "@/features/accounts/model";
import { call } from "@/lib/ipc";
import { resetAccountsMock } from "@/lib/mock";
import { resetDiscoverMock } from "@/lib/mockDiscover";
import { Shell, useApplyAppearance } from "./Shell";
import { useUi, type Route } from "./store";

// jsdom has no layout; give the virtual account list a viewport (as AccountsScreen.test does).
beforeAll(() => {
  for (const k of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"] as const)
    Object.defineProperty(HTMLElement.prototype, k, { configurable: true, get: () => 800 });
});

afterEach(() => {
  resetAccountsMock(0);
  resetDiscoverMock(false);
  useUi.setState({ sel: emptySel });
});

function InApp() {
  useApplyAppearance();
  return <Shell />;
}

/** Account names on screen: `leaks` outside `[data-pii]` (privacy mode can't hide them), `blurred`
 * inside. Text, input values and placeholders count; `title` always leaks (the native tooltip is
 * never blurred); `aria-label` counts unless inside `[data-pii]`. */
function scan(names: string[]) {
  const leaks: string[] = [];
  let blurred = 0;
  const check = (el: Element | null, text: string, where: string, always = false) => {
    const hit = names.find((name) => text.includes(name));
    if (!hit) return;
    if (!always && el?.closest("[data-pii]")) blurred++;
    else leaks.push(`${hit} in ${where} "${text.trim()}" (<${el?.tagName.toLowerCase()}>)`);
  };
  const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let n = walk.nextNode(); n; n = walk.nextNode())
    check(n.parentElement, n.textContent ?? "", "text");
  for (const el of document.body.querySelectorAll("*")) {
    for (const a of ["aria-label", "placeholder"])
      if (el.hasAttribute(a)) check(el, el.getAttribute(a)!, a);
    if (el.hasAttribute("title")) check(el, el.getAttribute("title")!, "title", true);
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)
      check(el, el.value, "value");
  }
  return { leaks, blurred };
}

// Every main screen with accounts, privacy mode on: no account name may show outside [data-pii],
// and the screens that list accounts must really have shown some (blurred).
test("privacy mode: no account name outside [data-pii] on the main screens", async () => {
  resetAccountsMock(6);
  resetDiscoverMock(true);
  await call("settings_set", { patch: { ui: { showUsernames: false } } });
  try {
    const { accounts } = await call("accounts_list");
    const names = [
      ...new Set(accounts.flatMap((a) => [a.displayName, a.username, a.alias ?? ""])),
    ].filter((s) => s.length >= 3);
    const screens: [string, Route, string[], boolean][] = [
      ["home", "home", [], true],
      ["accounts", "accounts", [], true],
      ["inspector", "accounts", [accounts[0]!.id], true],
      ["discover", "discover", [], false],
      ["devices", "devices", [], false],
      ["settings", "settings", [], false],
    ];
    const found: string[] = [];
    for (const [label, route, ids, mustShow] of screens) {
      useUi.setState({ route, sel: { ids, anchor: ids[0] ?? null, cursor: ids[0] ?? null } });
      const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      render(
        <QueryClientProvider client={qc}>
          <Tooltip.Provider>
            <InApp />
          </Tooltip.Provider>
        </QueryClientProvider>,
      );
      await waitFor(() => expect(document.documentElement.dataset.privacy).toBe("on"));
      await waitFor(() => expect(qc.isFetching()).toBe(0));
      const { leaks, blurred } = scan(names);
      found.push(...leaks.map((l) => `${label}: ${l}`));
      if (mustShow && blurred === 0) found.push(`${label}: no account names rendered`);
      cleanup();
    }
    expect(found).toEqual([]);
  } finally {
    await call("settings_set", { patch: { ui: { showUsernames: true } } });
  }
});
