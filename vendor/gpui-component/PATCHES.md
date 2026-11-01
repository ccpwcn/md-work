# md-work patches on gpui-component 0.8.0

## Search panel keystroke re-select (`src/input/overlay.rs`)

**Symptom:** In the editor Find bar (Ctrl+F / toolbar search), each typed
character was selected, so the next keystroke replaced it (`win` → `in` or `n`).

**Cause:** `SearchPanel` writes query changes back into `SearchSession` via
`set_search_query`, which also updates `anchor_offset`. The overlay echo guard
treated that anchor change as an external session update and called
`show_with_focus` → `select_all` after every `InputEvent::Change`.

**Fix:** Stop requiring `anchor_offset` equality in the `query_echo` guard.
Panel-owned query echoes are already identified by matching the panel input
against the session query (plus unchanged activation / replace mode).

**Remove when:** Upstream gpui-component (via gpui-kit) ships an equivalent fix;
then delete `[patch.crates-io]` in the workspace `Cargo.toml` and this vendor tree.
