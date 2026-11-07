---
name: md-work-dev-workflow
description: >-
  The build, test and packaging rules for the md-work (墨笺) repository. Use this
  whenever you change code, run builds or tests, or are about to produce a release
  artifact: every change must be compiled and tested with the dev runtime workspace,
  NSIS installers must never be built or installed until the user explicitly
  freezes features and asks for a package, and Releases are drafted and published
  only from main after dev is merged into it.
---

# md-work development workflow

These rules are mandatory for every session working on md-work.

## 1. Compile and test after every change

After each code change, run from the repository root:

```bash
cargo build --workspace
cargo test --workspace
```

- Use the normal `dev` profile. Do not switch to `--release` for routine checks.
- Manual runs use the developer runtime state (the workspace under
  `%LOCALAPPDATA%\md-work\` plus whatever workspace the developer has open).
  Do not point smoke tests at a release install or at the user's real notes
  without isolating them first (temporary `LOCALAPPDATA` + pre-seeded
  `config/runtime.toml` works well).
- When a GUI change touches runtime rendering, launch the dev binary and exercise
  the feature before reporting it done; type checking alone is not evidence.

## 2. Never build or install the NSIS package unless asked

- Do **not** run `cargo run -p xtask -- package`, `cargo run -p xtask -- installer`,
  `cargo run -p xtask -- all`, or `makensis` on your own initiative.
- Do **not** run the generated `dist\md-work-*-setup.exe`, and do not install,
  upgrade or uninstall it.
- The user personally tests the installer. Only build a package when the user says
  the features are frozen **and** explicitly asks for a package/release.
- The GitHub release workflow produces packages only on `main` (after `dev` is
  merged into it). A `v*` tag pushed on `dev` only signals "ready to release":
  it runs CI tests, never builds a package and never creates a Release there.
  Gitee Go still packages on tags (see `doc/ci.md`).

## 3. Keep silent install support

The NSIS script must keep working with `/S` (silent install/upgrade). If you touch
`packaging/nsis/md-work.nsi`:

- Keep `RequestExecutionLevel user` and the per-user HKCU-only install.
- Keep custom pages optional in silent mode: initialise every custom variable in
  `.onInit` because page callbacks are skipped under `/S`.
- Keep `QuietUninstallString` pointing at `uninstall.exe /S`.
- Never remove or rename `/S` support.
- Pass the chosen workspace through `HKCU\Software\md-work\WorkspaceRoot`
  (`WriteRegStr`) and never write config files from NSIS: `FileWrite` is ANSI, so
  non-ASCII paths would produce invalid UTF-8. The app reads this hint only while
  `config\runtime.toml` (or legacy `config.json`) has no workspace root, and
  persists the full config on first run.

## 4. Version bumps

- Single source of truth: root `Cargo.toml` `[workspace.package].version`.
- When the user asks to bump the version, run `.\scripts\bump-version.ps1`
  (explicit `X.Y.Z`, or `-Patch` / `-Minor` / `-Major`). Do not hand-edit
  version strings across files.
- Details: `.agents/skills/project-overview/references/version.md`.

## 5. Release surfaces

- Functional work happens on `dev`. Pushes to `dev` (with or without a `v*` tag)
  only build and test; **do not draft or publish a Release outside `main`**.
- Releases are drafted and published only on `main`: `.github/workflows/release.yml`
  runs on `main` pushes and on `v*` tag pushes (and `workflow_dispatch` from
  `main` only), requires that the `vX.Y.Z` tag matching `Cargo.toml` is already
  reachable from `main`, checks that the matching changelog is on `main`, and
  skips versions already published. A tag pushed from `dev` alone never
  releases; squash / rebase merges need the tag re-created on `main`.
- The tag must match `[workspace.package].version`, and
  `doc/changelog/vX.Y.Z.md` must exist and be non-empty; that file is the
  Release body (no auto-generated notes).
- Gitee Go still publishes on `v*` tags; push those tags to Gitee only after the
  `dev -> main` merge. See `doc/ci.md`.
- Do not push tags, merge into `main`, create releases, or publish installers
  unless the user asks.
