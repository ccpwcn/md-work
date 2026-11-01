---
name: md-work-dev-workflow
description: >-
  The build, test and packaging rules for the md-work (墨笺) repository. Use this
  whenever you change code, run builds or tests, or are about to produce a release
  artifact: every change must be compiled and tested with the dev runtime workspace,
  and NSIS installers must never be built or installed until the user explicitly
  freezes features and asks for a package.
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
- CI release workflows build artifacts on tags; that is the only other place a
  package is produced, and only when the user pushes a `v*` tag.

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

- Only a pushed `v*` tag triggers release packaging (GitHub Actions and Gitee Go
  where a self-hosted Windows host group is configured).
- The tag must match `[workspace.package].version`, and
  `doc/changelog/vX.Y.Z.md` must exist and be non-empty; that file is the
  Release body (no auto-generated notes).
- Do not push tags, create releases, or publish installers unless the user asks.
