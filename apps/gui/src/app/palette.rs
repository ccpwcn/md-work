use super::util::*;
use super::*;

impl AppShell {
    pub(super) fn palette_items(&self) -> Vec<PaletteItem> {
        let item = |key: &str, hint: &str, command: PaletteCommand| PaletteItem {
            label: t!(key).to_string(),
            hint: hint.to_string(),
            command,
        };
        vec![
            item("palette.new_note", "Ctrl+N", PaletteCommand::NewNote),
            item("palette.new_folder", "", PaletteCommand::NewFolder),
            item("palette.new_notebook", "", PaletteCommand::NewNotebook),
            item("palette.open_notebook", "", PaletteCommand::OpenNotebook),
            item("palette.save", "Ctrl+S", PaletteCommand::Save),
            item(
                "palette.toggle_preview",
                "Ctrl+T",
                PaletteCommand::TogglePreview,
            ),
            item("palette.close_tab", "", PaletteCommand::CloseTab),
            item("palette.settings", "", PaletteCommand::Settings),
            item("palette.search", "", PaletteCommand::Search),
            item("palette.outline", "", PaletteCommand::Outline),
            item("palette.bookmarks", "", PaletteCommand::Bookmarks),
            item("palette.trash", "", PaletteCommand::Trash),
            item("palette.notes_panel", "", PaletteCommand::NotesPanel),
            item("palette.export_pdf", "", PaletteCommand::ExportPdf),
            item("palette.export_webp", "", PaletteCommand::ExportWebp),
            item("palette.export_png", "", PaletteCommand::ExportPng),
            item("palette.export_zip", "", PaletteCommand::ExportNotebookZip),
            item("palette.import_zip", "", PaletteCommand::ImportNotebookZip),
            item("palette.move_note", "", PaletteCommand::MoveEntry),
            item("palette.theme_system", "", PaletteCommand::ThemeSystem),
            item("palette.theme_light", "", PaletteCommand::ThemeLight),
            item("palette.theme_dark", "", PaletteCommand::ThemeDark),
            item("palette.code_wrap", "", PaletteCommand::ToggleCodeWrap),
            item("palette.autosave", "", PaletteCommand::ToggleAutosave),
            item("palette.save_utf8", "", PaletteCommand::SaveAsUtf8),
            item("palette.save_gb18030", "", PaletteCommand::SaveAsGb18030),
            item(
                "palette.save_windows1252",
                "",
                PaletteCommand::SaveAsWindows1252,
            ),
            item("palette.save_utf16", "", PaletteCommand::SaveAsUtf16),
            item("palette.eol_lf", "", PaletteCommand::SaveEolLf),
            item("palette.eol_crlf", "", PaletteCommand::SaveEolCrlf),
            item(
                "palette.open_workspace",
                "",
                PaletteCommand::OpenWorkspaceFolder,
            ),
            item("palette.open_logs", "", PaletteCommand::OpenLogFolder),
            item(
                "palette.register_assoc",
                "",
                PaletteCommand::RegisterFileAssoc,
            ),
            item("palette.rescan", "", PaletteCommand::Rescan),
        ]
    }

    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("palette.placeholder").to_string())
                .clean_on_escape()
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.refresh_palette(cx);
            }
            InputEvent::PressEnter { .. } => {
                let handle = this.window_handle;
                let weak = cx.entity().downgrade();
                let _ = handle.update(cx, |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| this.run_palette_selected(window, cx));
                });
            }
            _ => {}
        })
        .detach();
        let items = self.palette_items();
        let filtered = (0..items.len()).collect();
        self.palette = Some(Palette {
            input,
            items,
            filtered,
            selected: 0,
        });
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn refresh_palette(&mut self, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        let query = palette.input.read(cx).value().trim().to_lowercase();
        let mut scored: Vec<(i64, usize)> = palette
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let score = fuzzy_score(&item.label.to_lowercase(), &query)?;
                Some((score, index))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        palette.filtered = scored.into_iter().map(|(_, index)| index).collect();
        palette.selected = 0;
        cx.notify();
    }

    pub(super) fn palette_step(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        if palette.filtered.is_empty() {
            return;
        }
        let len = palette.filtered.len() as i64;
        let next = (palette.selected as i64 + delta).rem_euclid(len);
        palette.selected = next as usize;
        cx.notify();
    }

    pub(super) fn run_palette_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_ref() else {
            return;
        };
        let command = palette
            .filtered
            .get(palette.selected)
            .and_then(|index| palette.items.get(*index))
            .map(|item| item.command);
        let Some(command) = command else {
            return;
        };
        self.close_palette(cx);
        self.run_palette_command(command, window, cx);
    }

    pub(super) fn run_palette_command(
        &mut self,
        command: PaletteCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            PaletteCommand::NewNote => self.new_note(window, cx),
            PaletteCommand::NewFolder => self.new_folder(window, cx),
            PaletteCommand::NewNotebook => self.new_notebook(cx),
            PaletteCommand::OpenNotebook => self.open_external_notebook_prompt(window, cx),
            PaletteCommand::Save => self.save_active(cx),
            PaletteCommand::TogglePreview => self.toggle_preview(window, cx),
            PaletteCommand::CloseTab => {
                if let Some(index) = self.active_doc {
                    self.close_doc(index, cx);
                }
            }
            PaletteCommand::Settings => self.open_settings(window, cx),
            PaletteCommand::Search => {
                self.set_panel(SidePanel::Search, cx);
                self.search_input
                    .update(cx, |state, cx| state.focus(window, cx));
            }
            PaletteCommand::Outline => self.set_panel(SidePanel::Outline, cx),
            PaletteCommand::Bookmarks => self.set_panel(SidePanel::Bookmarks, cx),
            PaletteCommand::Trash => self.set_panel(SidePanel::Trash, cx),
            PaletteCommand::NotesPanel => self.set_panel(SidePanel::Notes, cx),
            PaletteCommand::ExportPdf => {
                self.open_export_dialog_with(ExportFormat::Pdf, window, cx)
            }
            PaletteCommand::ExportPng => {
                self.open_export_dialog_with(ExportFormat::Png, window, cx)
            }
            PaletteCommand::ExportWebp => {
                self.open_export_dialog_with(ExportFormat::Webp, window, cx)
            }
            PaletteCommand::ExportNotebookZip => {
                if let Some(id) = self.active_notebook {
                    self.open_zip_export(id, window, cx);
                } else {
                    self.notify_warning(t!("zip.no_notebook").to_string(), cx);
                }
            }
            PaletteCommand::ImportNotebookZip => self.open_zip_import(window, cx),
            PaletteCommand::MoveEntry => {
                let target = self.active_doc_entity().and_then(|doc| {
                    let doc = doc.read(cx);
                    doc.notebook_id.map(|id| (id, doc.rel_path.clone()))
                });
                if let Some((notebook_id, rel_path)) = target {
                    self.begin_move(notebook_id, &rel_path, window, cx);
                } else {
                    self.notify_warning(t!("tree.move_external").to_string(), cx);
                }
            }
            PaletteCommand::ThemeSystem => {
                self.set_theme_preference(ThemePreference::System, window, cx)
            }
            PaletteCommand::ThemeLight => {
                self.set_theme_preference(ThemePreference::Light, window, cx)
            }
            PaletteCommand::ThemeDark => {
                self.set_theme_preference(ThemePreference::Dark, window, cx)
            }
            PaletteCommand::ToggleCodeWrap => self.toggle_code_wrap(cx),
            PaletteCommand::ToggleAutosave => self.set_autosave(!self.config.autosave, cx),
            PaletteCommand::SaveAsUtf8 => {
                let (_, _, eol) = self.current_encoding(cx);
                self.save_as_encoding(TextEncoding::Utf8, false, eol, cx)
            }
            PaletteCommand::SaveAsGb18030 => {
                let (_, _, eol) = self.current_encoding(cx);
                self.save_as_encoding(TextEncoding::Gb18030, false, eol, cx)
            }
            PaletteCommand::SaveAsWindows1252 => {
                let (_, _, eol) = self.current_encoding(cx);
                self.save_as_encoding(TextEncoding::Windows1252, false, eol, cx)
            }
            PaletteCommand::SaveAsUtf16 => {
                let (_, _, eol) = self.current_encoding(cx);
                self.save_as_encoding(TextEncoding::Utf16Le, true, eol, cx)
            }
            PaletteCommand::SaveEolLf => {
                let (encoding, bom, _) = self.current_encoding(cx);
                self.save_as_encoding(encoding, bom, Eol::Lf, cx)
            }
            PaletteCommand::SaveEolCrlf => {
                let (encoding, bom, _) = self.current_encoding(cx);
                self.save_as_encoding(encoding, bom, Eol::Crlf, cx)
            }
            PaletteCommand::OpenWorkspaceFolder => {
                let root = self
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.root().to_path_buf());
                self.open_path_in_explorer(root, cx);
            }
            PaletteCommand::OpenLogFolder => {
                self.open_path_in_explorer(Some(mdwork_core::paths::logs_dir()), cx)
            }
            PaletteCommand::RegisterFileAssoc => self.register_file_assoc(cx),
            PaletteCommand::Rescan => {
                self.reload(cx);
                self.persist_session(cx);
                cx.notify();
            }
        }
    }
}
