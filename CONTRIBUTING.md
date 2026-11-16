# Contributing

Thanks for wanting to help. Anything goes: bug fixes, features, UI polish, docs, tests. Small PRs get merged fastest.

## Setup

You need Windows 10/11, macOS or Linux, Rust stable (MSVC toolchain on Windows), Node 22+ and pnpm through corepack. On Linux also install the Tauri system packages listed in `.github/workflows/release.yml`.

```bash
git clone https://github.com/KingIcyCreamProjects/Roblox-Multi-Instance.git
cd Roblox-Multi-Instance
corepack enable
corepack pnpm i
corepack pnpm tauri dev
```

`corepack pnpm dev` runs just the UI in a browser with mock data (fake accounts, real public game info), which is handy for UI work since it never touches Roblox or your vault.

## Making a change

1. Fork, then branch off `main` (`fix/server-list-sort`, `feat/hotkeys`, whatever describes it).
2. Keep it focused. One fix or feature per PR is easier to review than a grab bag.
3. Add or update a test when you change behavior. Frontend tests live next to the component (`*.test.tsx`, vitest), Rust tests in `#[cfg(test)]` modules.
4. Run the checks below. CI runs the same ones on Windows, macOS and Linux.
5. Open the PR against `main` and say what changed and how you tested it. Screenshots help for anything visual.

For something big (a new screen, a new dependency, changes to how the vault or launching works), open an issue first so we can agree on the approach before you sink time into it.

## Checks

```bash
corepack pnpm lint
corepack pnpm typecheck
corepack pnpm test
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## A few rules

- Never commit secrets: cookies, tokens, the updater signing key, real account names in screenshots.
- New Tauri commands need an entry in `src-tauri/capabilities/default.json` and `build.rs`; `src/lib/acl.test.ts` fails if you forget.
- User-facing text goes in `src/lib/i18n.ts`. Keep it short and plain.
- Anything that shows an account name in the UI needs `data-pii` so privacy mode can blur it.

## Reporting bugs

Open an issue with what you did, what you expected, what happened, your Windows version, and the RMI log from `%APPDATA%\Roblox Multi Instance\logs` (cookies and tokens are redacted, but read it before you post).

By contributing you agree your work is released under the [MIT License](LICENSE).
