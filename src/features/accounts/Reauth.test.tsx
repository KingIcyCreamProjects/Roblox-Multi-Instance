import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import * as ipc from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { ReauthSheet, requestReauth } from "./Reauth";

const orig = ipc.call;
/** each `reauth` call waits here until the test settles it */
let answers: { ok: () => void; fail: (e: unknown) => void }[] = [];

beforeEach(() => {
  answers = [];
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) => {
    if (cmd === "vault_status")
      return Promise.resolve({ exists: true, unlocked: true, mode: "password" } as never);
    if (cmd === "reauth")
      return new Promise((ok, fail) => answers.push({ ok: () => ok(undefined as never), fail }));
    return orig(cmd, args as never);
  });
});
afterEach(() => vi.restoreAllMocks());

function setup() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <Tooltip.Provider>
        <ReauthSheet />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
}

const wrong = () => ipc.errMessage("WRONG_PASSWORD");

async function submit(user: ReturnType<typeof userEvent.setup>, pw: string) {
  await user.type(await screen.findByLabelText(t("vault.password")), pw);
  await user.click(screen.getByRole("button", { name: t("reauth.confirm") }));
}

test("a wrong password keeps the sheet open; success settles every shared request true", async () => {
  const user = userEvent.setup();
  setup();
  let first!: Promise<boolean>;
  act(() => void (first = requestReauth()));
  expect(requestReauth()).toBe(first);
  await submit(user, "nope");
  await act(async () => answers[0].fail("WRONG_PASSWORD"));
  expect(await screen.findByText(wrong())).toBeInTheDocument();
  await user.clear(screen.getByLabelText(t("vault.password")));
  await submit(user, "right");
  await act(async () => answers[1].ok());
  await expect(first).resolves.toBe(true);
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("closed mid-check: resolves false, and the late error never shows on the next request", async () => {
  const user = userEvent.setup();
  setup();
  let first!: Promise<boolean>;
  act(() => void (first = requestReauth()));
  await submit(user, "nope");
  await user.keyboard("{Escape}");
  await expect(first).resolves.toBe(false);
  let second!: Promise<boolean>;
  act(() => void (second = requestReauth()));
  expect(second).not.toBe(first);
  await act(async () => answers[0].fail("WRONG_PASSWORD"));
  await screen.findByLabelText(t("vault.password"));
  expect(screen.queryByText(wrong())).not.toBeInTheDocument();
  await user.keyboard("{Escape}");
  await expect(second).resolves.toBe(false);
});

test("unmount (vault lock) settles an open request false", async () => {
  const { unmount } = setup();
  let p!: Promise<boolean>;
  act(() => void (p = requestReauth()));
  unmount();
  await expect(p).resolves.toBe(false);
});
