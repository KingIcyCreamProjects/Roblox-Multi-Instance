import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useUi } from "@/app/store";
import { useAccountEvents } from "@/features/accounts/useAccounts";
import * as mock from "@/lib/mock";
import { mockImport, mockVault, resetAccountsMock } from "@/lib/mock";
import { t } from "@/lib/i18n";
import { ImportSheet } from "./ImportSheet";

// jsdom has no layout; give the virtual list a viewport.
beforeAll(() => {
  for (const k of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"] as const)
    Object.defineProperty(HTMLElement.prototype, k, { configurable: true, get: () => 800 });
});

beforeEach(() => {
  resetAccountsMock(3);
  useUi.setState({ importOpen: true });
});

function Harness() {
  useAccountEvents();
  return <ImportSheet />;
}

function setup() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <Harness />
    </QueryClientProvider>,
  );
  return userEvent.setup();
}

test("locked file: folder, password, preview, import, results with Sign in again", async () => {
  mockImport.encoding = "locked";
  const user = setup();
  await user.click(screen.getByRole("button", { name: t("imp.browse") }));
  expect(await screen.findByText(t("imp.enc.locked"))).toBeInTheDocument();
  expect(screen.getByLabelText(t("imp.folder"))).toHaveValue("C:\\Users\\me\\OldManager");

  const pw = screen.getByLabelText(t("imp.pw"));
  await user.type(pw, "nope");
  await user.click(screen.getByRole("button", { name: t("imp.preview") }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Wrong old password");

  await user.clear(pw);
  await user.type(pw, "oldpass");
  await user.click(screen.getByRole("button", { name: t("imp.preview") }));
  const list = await screen.findByRole("region", { name: t("imp.list") });
  expect(screen.queryByLabelText(t("imp.pw"))).toBeNull();
  expect(within(list).getAllByText(t("imp.willUpdate"))).toHaveLength(1);
  expect(within(list).getAllByRole("listitem")[0]).toHaveAttribute("aria-setsize", "3");
  expect(mockVault.lastArgs.import_preview).toEqual({
    path: "C:\\Users\\me\\OldManager",
    password: "oldpass",
    accounts: true,
  });

  await user.click(screen.getByRole("button", { name: t("imp.noneSel") }));
  expect(screen.getByRole("button", { name: t("imp.ctaOther") })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("imp.all") }));
  await user.click(screen.getByRole("button", { name: t("imp.cta", { n: 3 }) }));

  const res = await screen.findByRole("list", { name: t("imp.results") });
  expect(within(res).getAllByText(t("imp.res.added"))).toHaveLength(2);
  expect(within(res).getByText(t("imp.res.updated"))).toBeInTheDocument();
  expect(
    await within(res).findByRole("button", { name: t("imp.signInAgain") }),
  ).toBeInTheDocument();
  expect(screen.getByText("Settings imported.")).toBeInTheDocument();
  expect(mockVault.lastArgs.import_commit).toMatchObject({
    previewId: "pv-1",
    importPrefs: true,
    importRecents: true,
    importFavorites: false,
  });
});

test("unlocked file skips the password step", async () => {
  mockImport.encoding = "dpapi";
  const user = setup();
  await user.type(screen.getByLabelText(t("imp.folder")), "C:\\old");
  await user.click(screen.getByRole("button", { name: t("imp.look") }));
  expect(await screen.findByText(t("imp.enc.dpapi"))).toBeInTheDocument();
  expect(screen.queryByLabelText(t("imp.pw"))).toBeNull();
  await user.click(screen.getByRole("button", { name: t("imp.preview") }));
  expect(await screen.findByRole("button", { name: t("imp.cta", { n: 3 }) })).toBeInTheDocument();
  expect(mockVault.lastArgs.import_preview).toEqual({
    path: "C:\\old",
    password: null,
    accounts: true,
  });
});

test("expired preview returns to Show accounts", async () => {
  mockImport.encoding = "plain";
  mockImport.expire = true;
  try {
    const user = setup();
    await user.click(screen.getByRole("button", { name: t("imp.browse") }));
    await user.click(await screen.findByRole("button", { name: t("imp.preview") }));
    await user.click(await screen.findByRole("button", { name: t("imp.cta", { n: 3 }) }));
    expect(await screen.findByRole("alert")).toHaveTextContent("expired");
    expect(screen.getByRole("button", { name: t("imp.preview") })).toBeInTheDocument();
    const items = screen.queryAllByRole("listitem");
    expect(items).toHaveLength(0);
  } finally {
    mockImport.expire = false;
  }
});

test("unticked Accounts never sends the account file for decrypt", async () => {
  mockImport.encoding = "dpapi";
  const user = setup();
  await user.click(screen.getByRole("button", { name: t("imp.browse") }));
  await user.click(await screen.findByRole("checkbox", { name: new RegExp(t("imp.f.accounts")) }));
  await user.click(screen.getByRole("button", { name: t("imp.preview") }));
  expect(await screen.findByRole("button", { name: t("imp.ctaOther") })).toBeInTheDocument();
  expect(mockVault.lastArgs.import_preview).toMatchObject({ accounts: false });
  expect(screen.queryByRole("region", { name: t("imp.list") })).toBeNull();
});

test("an unreadable accounts file is named and cannot be ticked", async () => {
  const orig = mock.mockCall;
  const spy = vi.spyOn(mock, "mockCall").mockImplementation((cmd, args) =>
    cmd === "import_detect"
      ? Promise.resolve({
          accountData: { encoding: null, error: "Could not read the accounts file." },
          settings: false,
          recents: false,
          favorites: false,
        })
      : orig(cmd, args),
  );
  const user = setup();
  await user.click(screen.getByRole("button", { name: t("imp.browse") }));
  expect(await screen.findByText("Could not read the accounts file.")).toBeInTheDocument();
  expect(screen.getByRole("checkbox", { name: new RegExp(t("imp.f.accounts")) })).toBeDisabled();
  spy.mockRestore();
});

test("a settings/games failure offers Try again, back to the kept preview", async () => {
  mockImport.encoding = "plain";
  mockImport.retry = true;
  try {
    const user = setup();
    await user.click(screen.getByRole("button", { name: t("imp.browse") }));
    await user.click(await screen.findByRole("button", { name: t("imp.preview") }));
    await user.click(await screen.findByRole("button", { name: t("imp.cta", { n: 3 }) }));
    await user.click(await screen.findByRole("button", { name: t("imp.retry") }));
    expect(screen.getByRole("button", { name: t("imp.cta", { n: 3 }) })).toBeInTheDocument();
  } finally {
    mockImport.retry = false;
  }
});

test("Esc mid-step keeps the sheet open and says why; the step result still lands", async () => {
  mockImport.encoding = "dpapi";
  const orig = mock.mockCall;
  let release = () => {};
  const spy = vi
    .spyOn(mock, "mockCall")
    .mockImplementation((cmd, args) =>
      cmd === "import_detect"
        ? new Promise((r) => (release = () => r(orig(cmd, args))))
        : orig(cmd, args),
    );
  const user = setup();
  await user.type(screen.getByLabelText(t("imp.folder")), "C:\\old");
  await user.click(screen.getByRole("button", { name: t("imp.look") }));
  await user.keyboard("{Escape}");
  expect(await screen.findByText(t("imp.busy"))).toBeInTheDocument();
  expect(useUi.getState().importOpen).toBe(true);
  release();
  expect(await screen.findByText(t("imp.enc.dpapi"))).toBeInTheDocument();
  expect(screen.queryByText(t("imp.busy"))).toBeNull();
  spy.mockRestore();
});

test("one ticked account labels the import button for one", async () => {
  mockImport.encoding = "plain";
  const user = setup();
  await user.click(screen.getByRole("button", { name: t("imp.browse") }));
  await user.click(await screen.findByRole("button", { name: t("imp.preview") }));
  await user.click(await screen.findByRole("button", { name: t("imp.noneSel") }));
  await user.click(screen.getByRole("checkbox", { name: "Old main" }));
  expect(await screen.findByRole("button", { name: t("imp.cta1") })).toBeInTheDocument();
});
