import { useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import clsx from "clsx";
import { CircleAlert, CircleCheck, FolderOpen, LogIn } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { useUi } from "@/app/store";
import { Button } from "@/components/ui/Button";
import { Sheet } from "@/components/ui/Sheet";
import {
  call,
  errMessage,
  fileSrc,
  hasCode,
  type ImportDetect,
  type ImportPreview,
  type ImportResult,
  type ImportRow,
} from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { KEY, useAccountActions, useAccounts } from "@/features/accounts/useAccounts";

type Use = { accounts: boolean; settings: boolean; recents: boolean; favorites: boolean };
type Done = { results: ImportResult[]; notes: string[]; retry: boolean };

const input =
  "h-9 w-full rounded-control bg-field shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus:shadow-[var(--focus-ring)] px-3 text-13 text-text-1 placeholder:text-text-3";
const ROW_H = 48;
/** Rust waits up to 5 min for a validation slot, then the check itself runs: allow for it. */
const CHECK_MS = 360_000;

/**
 * One import step (detect, preview, commit) at a time app-wide: Rust keeps a single preview
 * slot, and a step from a closed sheet must not overlap one from a reopened sheet.
 */
let inFlight = false;

/** Spec §11.4: folder → detected files → old password (locked only) → preview → import → results. */
export function ImportSheet() {
  const open = useUi((u) => u.importOpen);
  // a close was refused mid-step: say why until the step ends
  const [refused, setRefused] = useState(false);
  return (
    <Sheet
      open={open}
      // no close (X, Esc, overlay) mid-step: the flow would unmount and drop the results
      onOpenChange={(o) => {
        if (o || !inFlight) useUi.setState({ importOpen: o });
        else setRefused(true);
      }}
      title={t("imp.title")}
      description={t("imp.body")}
      width={680}
      centered
    >
      {/* content unmounts on close, so every open starts fresh and the password is gone */}
      <Flow
        onClose={() => useUi.setState({ importOpen: false })}
        refused={refused}
        onSettled={() => setRefused(false)}
      />
    </Sheet>
  );
}

function Flow({
  onClose,
  refused,
  onSettled,
}: {
  onClose: () => void;
  refused: boolean;
  onSettled: () => void;
}) {
  const qc = useQueryClient();
  const pathId = useId();
  const pwId = useId();
  const [path, setPath] = useState("");
  const [found, setFound] = useState<ImportDetect | null>(null);
  const [use, setUse] = useState<Use>({
    accounts: false,
    settings: false,
    recents: false,
    favorites: false,
  });
  // old manager's password: sent once to import_preview, then cleared
  const [pw, setPw] = useState("");
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState<"look" | "read" | "import" | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [done, setDone] = useState<Done | null>(null);

  const run = async (kind: "look" | "read" | "import", f: () => Promise<void>) => {
    if (inFlight) return void setErr(t("imp.busy")); // a step from an earlier open still runs
    inFlight = true;
    setBusy(kind);
    setErr(null);
    try {
      await f();
    } catch (e) {
      setErr(errMessage(e));
    } finally {
      inFlight = false;
      setBusy(null);
      onSettled();
    }
  };
  const look = (p: string) =>
    run("look", async () => {
      setFound(null);
      setPreview(null);
      const d = await call("import_detect", { path: p });
      setFound(d);
      setUse({
        accounts: !!d.accountData?.encoding,
        settings: d.settings,
        recents: d.recents,
        favorites: d.favorites,
      });
    });
  // the folder dialog is open: a second click must not open another
  const [picking, setPicking] = useState(false);
  const browse = async () => {
    setPicking(true);
    try {
      const r = await call("import_pick_folder");
      if (r) {
        setPath(r.path);
        void look(r.path);
      }
    } catch (e) {
      setErr(errMessage(e));
    } finally {
      setPicking(false);
    }
  };

  const enc = found?.accountData?.encoding ?? null;
  const needPw = use.accounts && enc === "locked";
  const read = () =>
    run("read", async () => {
      let r: ImportPreview;
      try {
        r = await call("import_preview", {
          path: path.trim(),
          password: needPw ? pw : null,
          accounts: use.accounts,
        });
      } finally {
        setPw(""); // forgotten after every read, failed or not
      }
      setPreview(r);
      setSel(new Set(r.rows.map((x) => x.key)));
    });
  const n = use.accounts && preview ? sel.size : 0;
  const commit = () =>
    run("import", async () => {
      const r = await call("import_commit", {
        previewId: preview!.previewId,
        keys: use.accounts ? [...sel] : [],
        importPrefs: use.settings,
        importRecents: use.recents,
        importFavorites: use.favorites,
      }).catch((e: unknown) => {
        // Rust dropped the decrypted set: back to "Show accounts" (password again if locked)
        if (hasCode(e, "PREVIEW_EXPIRED")) setPreview(null); // import/cmds.rs EXPIRED, 5 min
        throw e;
      });
      setDone(r);
      void qc.invalidateQueries({ queryKey: KEY });
      if (use.settings) void qc.invalidateQueries({ queryKey: ["settings"] });
    });

  if (done) return <Results done={done} onClose={onClose} onRetry={() => setDone(null)} />;

  const files: [keyof Use, MsgKey, boolean, string | null][] = found
    ? [
        [
          "accounts",
          "imp.f.accounts",
          !!found.accountData,
          found.accountData?.error ??
            (enc &&
              [t(`imp.enc.${enc}`), preview && t("imp.f.count", { n: preview.rows.length })]
                .filter(Boolean)
                .join(" ")),
        ],
        ["settings", "imp.f.settings", found.settings, null],
        [
          "recents",
          "imp.f.recents",
          found.recents,
          preview ? t("imp.f.count", { n: preview.games.recents }) : null,
        ],
        [
          "favorites",
          "imp.f.favorites",
          found.favorites,
          preview ? t("imp.f.count", { n: preview.games.favorites }) : null,
        ],
      ]
    : [];
  const shown = files.filter((f) => f[2]);
  const anyUse = use.accounts || use.settings || use.recents || use.favorites;

  return (
    <div className="flex flex-col gap-6">
      <form
        className="flex flex-col gap-1.5"
        onSubmit={(e) => {
          e.preventDefault();
          if (path.trim()) void look(path.trim());
        }}
      >
        <label htmlFor={pathId} className="text-13 font-medium text-text-1">
          {t("imp.folder")}
        </label>
        <div className="flex gap-2">
          <input
            id={pathId}
            // the folder path usually holds the Windows user name
            data-pii
            value={path}
            onChange={(e) => {
              setPath(e.target.value);
              setFound(null);
              setPreview(null);
            }}
            spellCheck={false}
            autoComplete="off"
            placeholder={t("imp.folderPlaceholder")}
            className={clsx(input, "font-mono text-12")}
            // fixed while any step runs: a detect or preview can't land on a newer folder
            disabled={!!busy}
          />
          <Button onClick={() => void browse()} className="shrink-0" disabled={!!busy || picking}>
            <FolderOpen size={16} strokeWidth={1.75} />
            {t("imp.browse")}
          </Button>
        </div>
        {path.trim() && !found && (
          <Button type="submit" className="mt-1 self-start" disabled={!!busy}>
            {busy === "look" ? t("imp.looking") : t("imp.look")}
          </Button>
        )}
      </form>

      {found && (
        <section aria-label={t("imp.found")} className="flex flex-col gap-2">
          <h3 className="text-13 font-medium text-text-1">{t("imp.found")}</h3>
          {shown.length === 0 && <p className="text-13 text-text-2">{t("imp.none")}</p>}
          {shown.map(([k, label, , detail]) => {
            const broken = k === "accounts" && !enc;
            return (
              <label key={k} className="flex items-start gap-2.5 text-13">
                <input
                  type="checkbox"
                  className="mt-0.5 size-4 accent-[var(--accent)]"
                  checked={use[k]}
                  disabled={broken || !!preview || !!busy}
                  onChange={(e) => setUse({ ...use, [k]: e.target.checked })}
                />
                <span className="min-w-0 flex-1">
                  <span className="text-text-1">{t(label)}</span>
                  {detail && (
                    <span className={clsx("block", broken ? "text-error" : "text-text-2")}>
                      {detail}
                    </span>
                  )}
                </span>
              </label>
            );
          })}
          {preview?.notes.map((m) => (
            <p key={m} className="text-13 text-warn">
              {m}
            </p>
          ))}
        </section>
      )}

      {found && shown.length > 0 && !preview && (
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            void read();
          }}
        >
          {needPw && (
            <div className="flex flex-col gap-1.5">
              <label htmlFor={pwId} className="text-13 font-medium text-text-1">
                {t("imp.pw")}
              </label>
              <input
                id={pwId}
                type="password"
                value={pw}
                onChange={(e) => setPw(e.target.value)}
                autoComplete="off"
                spellCheck={false}
                autoFocus
                className={input}
              />
              <p className="text-12 text-text-3">{t("imp.pwNote")}</p>
            </div>
          )}
          <Button
            type="submit"
            variant="primary"
            className="self-end"
            disabled={!anyUse || busy === "read" || (needPw && !pw)}
          >
            {busy === "read" ? t("imp.reading") : t("imp.preview")}
          </Button>
          {busy === "read" && needPw && (
            <p role="status" className="text-13 text-text-2">
              {t("imp.readingLocked")}
            </p>
          )}
        </form>
      )}

      {preview && use.accounts && <Pick rows={preview.rows} sel={sel} setSel={setSel} />}

      {err && (
        <p role="alert" className="text-13 text-error">
          {err}
        </p>
      )}
      {refused && busy && (
        <p role="status" className="text-13 text-text-2">
          {t("imp.busy")}
        </p>
      )}

      {preview && (
        <Button
          variant="primary"
          className="self-end"
          disabled={
            busy === "import" || (n === 0 && !use.settings && !use.recents && !use.favorites)
          }
          onClick={() => void commit()}
        >
          {busy === "import"
            ? t("imp.importing")
            : n === 1
              ? t("imp.cta1")
              : n
                ? t("imp.cta", { n })
                : t("imp.ctaOther")}
        </Button>
      )}
    </div>
  );
}

const initials = (r: ImportRow) =>
  (r.alias || r.username)
    .replace(/[^A-Za-z0-9]/g, "")
    .slice(0, 2)
    .toUpperCase();

/** Preview table: virtualized, so thousands of legacy rows stay smooth. */
function Pick({
  rows,
  sel,
  setSel,
}: {
  rows: ImportRow[];
  sel: Set<string>;
  setSel: (s: Set<string>) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => ref.current,
    estimateSize: () => ROW_H,
    overscan: 8,
  });
  const toggle = (k: string) => {
    const s = new Set(sel);
    if (!s.delete(k)) s.add(k);
    setSel(s);
  };
  if (!rows.length) return <p className="text-13 text-text-2">{t("imp.noAccounts")}</p>;
  return (
    <section aria-label={t("imp.list")} className="flex flex-col gap-2">
      <div className="flex items-center gap-3">
        <h3 className="flex-1 text-13 font-medium text-text-1">
          {t("imp.list")}{" "}
          <span className="font-normal text-text-3 tnum">
            {t("imp.selCount", { n: sel.size, total: rows.length })}
          </span>
        </h3>
        <Button variant="ghost" onClick={() => setSel(new Set(rows.map((r) => r.key)))}>
          {t("imp.all")}
        </Button>
        <Button variant="ghost" onClick={() => setSel(new Set())}>
          {t("imp.noneSel")}
        </Button>
      </div>
      <div
        ref={ref}
        className="overflow-auto rounded-control bg-surface-1"
        style={{ height: Math.min(rows.length * ROW_H, 360) }}
      >
        <ul className="relative" style={{ height: v.getTotalSize() }}>
          {v.getVirtualItems().map((vi) => {
            const r = rows[vi.index];
            const src = fileSrc(r.headshot);
            return (
              <li
                key={r.key}
                aria-setsize={rows.length}
                aria-posinset={vi.index + 1}
                className="absolute inset-x-0 top-0 flex items-center gap-3 border-t border-hairline px-3 first:border-t-0"
                style={{ height: ROW_H, transform: `translateY(${vi.start}px)` }}
              >
                <input
                  type="checkbox"
                  aria-label={r.alias || r.username}
                  checked={sel.has(r.key)}
                  onChange={() => toggle(r.key)}
                  className="size-4 accent-[var(--accent)]"
                />
                <span
                  data-pii
                  aria-hidden
                  className="inline-grid size-7 shrink-0 place-items-center overflow-hidden rounded-control bg-surface-4 text-[10px] font-semibold text-text-2"
                >
                  {src ? <img src={src} alt="" className="size-full object-cover" /> : initials(r)}
                </span>
                <span data-pii className="min-w-0 flex-1 truncate text-13 text-text-1">
                  {r.alias || r.username}
                  {r.alias && <span className="text-text-3"> @{r.username}</span>}
                </span>
                {r.willUpdate && (
                  <span className="shrink-0 rounded-full bg-surface-4 px-2 py-0.5 text-12 font-medium text-text-2">
                    {t("imp.willUpdate")}
                  </span>
                )}
                <span data-pii className="w-24 shrink-0 truncate text-right text-12 text-text-3">
                  {r.group}
                </span>
              </li>
            );
          })}
        </ul>
      </div>
    </section>
  );
}

const resKey: Record<ImportResult["status"], MsgKey> = {
  added: "imp.res.added",
  updated: "imp.res.updated",
  skipped: "imp.res.skipped",
  failed: "imp.res.failed",
};

/** Per-account results; the session check after import fills in live (accounts://changed). */
function Results({
  done,
  onClose,
  onRetry,
}: {
  done: Done;
  onClose: () => void;
  /** Back to the preview (Rust kept it after a settings or games failure). */
  onRetry: () => void;
}) {
  const { data } = useAccounts();
  const { signIn } = useAccountActions();
  const byId = new Map(data?.accounts.map((a) => [a.id, a]));
  const [gaveUp, setGaveUp] = useState(false);
  useEffect(() => {
    const h = setTimeout(() => setGaveUp(true), CHECK_MS);
    return () => clearTimeout(h);
  }, []);
  return (
    <div className="flex flex-col gap-4">
      <ul aria-label={t("imp.results")} className="flex flex-col">
        {done.results.map((r) => {
          const ok = r.status === "added" || r.status === "updated";
          const a = r.id ? byId.get(r.id) : undefined;
          return (
            <li
              key={r.key}
              className="flex items-start gap-2.5 border-t border-hairline py-2 text-13 first:border-t-0"
            >
              {ok ? (
                <CircleCheck size={15} strokeWidth={1.75} className="mt-0.5 shrink-0 text-online" />
              ) : (
                <CircleAlert
                  size={15}
                  strokeWidth={1.75}
                  className={clsx(
                    "mt-0.5 shrink-0",
                    r.status === "failed" ? "text-error" : "text-text-3",
                  )}
                />
              )}
              <span className="min-w-0 flex-1">
                <span data-pii className="text-text-1">
                  @{r.username}
                </span>{" "}
                <span className="text-text-2">{t(resKey[r.status])}</span>
                {r.message && <span className="block text-text-2">{r.message}</span>}
              </span>
              {a?.session === "unknown" && (
                <span className="shrink-0 text-12 text-text-3">
                  {gaveUp ? t("session.unknown") : t("imp.checking")}
                </span>
              )}
              {a?.session === "ok" && (
                <span className="shrink-0 text-12 text-online">{t("session.ok")}</span>
              )}
              {(a?.session === "expired" || a?.session === "pending") && (
                <Button className="-my-1 shrink-0" onClick={() => void signIn(a.id, a.username)}>
                  <LogIn size={14} strokeWidth={2} />
                  {a.session === "expired" ? t("imp.signInAgain") : t("imp.signIn")}
                </Button>
              )}
            </li>
          );
        })}
      </ul>
      {done.notes.map((m) => (
        <p key={m} className="text-13 text-text-2">
          {m}
        </p>
      ))}
      <div className="flex justify-end gap-2">
        {done.retry && <Button onClick={onRetry}>{t("imp.retry")}</Button>}
        <Button variant="primary" onClick={onClose}>
          {t("imp.done")}
        </Button>
      </div>
    </div>
  );
}
