use super::util::*;
use super::*;

impl AppShell {
    pub(super) fn open_export_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_export_dialog_with(ExportFormat::Pdf, window, cx);
    }

    pub(super) fn open_export_dialog_with(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.export.is_some() {
            return;
        }
        let Some(doc_entity) = self.active_doc_entity().cloned() else {
            self.notify_warning(t!("export.no_doc").to_string(), cx);
            return;
        };
        let (notebook_id, notebook_name, notebook_root, note_rel, title, markdown) = {
            let doc = doc_entity.read(cx);
            let notebook_id = doc.notebook_id;
            let notebook = notebook_id.and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
            });
            let (name, root) = match notebook {
                Some(entry) => (
                    entry.notebook.display_name().to_string(),
                    entry.notebook.root.clone(),
                ),
                None => (
                    String::new(),
                    doc.path.parent().map(Path::to_path_buf).unwrap_or_default(),
                ),
            };
            let note_rel = if notebook_id.is_some() {
                doc.rel_path.clone()
            } else {
                doc.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| doc.rel_path.clone())
            };
            (
                notebook_id,
                name,
                root,
                note_rel,
                doc.title(),
                doc.text(cx).to_string(),
            )
        };

        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let fallback = notebook_root.clone();
            let prefer_pdf_dir = format == ExportFormat::Pdf;
            let (prefs, dir) = cx
                .background_executor()
                .spawn(async move {
                    let prefs = notebook_id
                        .map(|id| NotebookPrefs::load(&id))
                        .unwrap_or_default();
                    let dir = prefs.export_dir(prefer_pdf_dir).unwrap_or(fallback);
                    (prefs, dir)
                })
                .await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.open_export_dialog_ready(
                        format,
                        notebook_id,
                        notebook_name,
                        notebook_root,
                        note_rel,
                        title,
                        markdown,
                        prefs,
                        dir,
                        window,
                        cx,
                    );
                });
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_export_dialog_ready(
        &mut self,
        format: ExportFormat,
        notebook_id: Option<Uuid>,
        notebook_name: String,
        notebook_root: PathBuf,
        note_rel: String,
        title: String,
        markdown: String,
        prefs: NotebookPrefs,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.export.is_some() {
            return;
        }
        let options = prefs.export.clone();
        let stem = Path::new(&note_rel)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "export".to_string());
        let output = dir.join(format!("{stem}.{}", format.extension()));

        let header_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(options.header.clone())
                .placeholder(t!("export.header_placeholder").to_string())
                .clean_on_escape()
        });
        let footer_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(options.footer.clone())
                .placeholder(t!("export.footer_placeholder").to_string())
                .clean_on_escape()
        });
        let output_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(output.display().to_string())
                .clean_on_escape()
        });

        self.export = Some(ExportDialog {
            notebook_id,
            notebook_root,
            notebook_name,
            note_rel,
            title,
            markdown,
            format,
            options,
            open_after_export: prefs.open_after_export,
            header_input,
            footer_input,
            output_input,
            job: None,
            busy: false,
            confirm_overwrite: false,
            stage: None,
            progress: None,
            warnings: Vec::new(),
            error: None,
        });
        cx.notify();
    }

    pub(super) fn close_export_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.export.take() {
            if let Some(job) = dialog.job {
                job.cancel();
            }
        }
        cx.notify();
    }

    pub(super) fn set_export_option(
        &mut self,
        apply: impl FnOnce(&mut ExportOptions),
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.export.as_mut() {
            apply(&mut dialog.options);
            dialog.confirm_overwrite = false;
            cx.notify();
        }
    }

    pub(super) fn set_export_format(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.export.as_mut() else {
            return;
        };
        if dialog.format == format || dialog.busy {
            return;
        }
        dialog.format = format;
        dialog.confirm_overwrite = false;
        let current = PathBuf::from(dialog.output_input.read(cx).value().to_string());
        let next = ensure_export_extension(current, format);
        dialog.output_input.update(cx, |state, cx| {
            state.set_value(next.display().to_string(), window, cx)
        });
        cx.notify();
    }

    pub(super) fn pick_export_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.export.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let current = PathBuf::from(dialog.output_input.read(cx).value().to_string());
        let dir = current
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dialog.notebook_root.clone());
        let name = current
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("export.{}", dialog.format.extension()));
        let receiver = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = receiver.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    if let Some(dialog) = this.export.as_mut() {
                        let path = ensure_export_extension(path, dialog.format);
                        let display = path.display().to_string();
                        dialog.confirm_overwrite = false;
                        dialog
                            .output_input
                            .update(cx, |state, cx| state.set_value(display, window, cx));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn begin_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.export.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let cli = match self.cli.clone() {
            Ok(path) => path,
            Err(message) => {
                if let Some(dialog) = self.export.as_mut() {
                    dialog.error = Some(message);
                }
                cx.notify();
                return;
            }
        };
        let raw_output = PathBuf::from(dialog.output_input.read(cx).value().to_string());
        if raw_output.file_name().is_none() {
            if let Some(dialog) = self.export.as_mut() {
                dialog.error = Some(t!("export.invalid_output").to_string());
            }
            cx.notify();
            return;
        }
        let format = dialog.format;
        let output = ensure_export_extension(raw_output, format);
        let confirmed = dialog.confirm_overwrite;
        if export_target_exists(&output) && !confirmed {
            if let Some(dialog) = self.export.as_mut() {
                dialog.confirm_overwrite = true;
                dialog.error = None;
                let display = output.display().to_string();
                dialog
                    .output_input
                    .update(cx, |state, cx| state.set_value(display, window, cx));
            }
            cx.notify();
            return;
        }
        let mut options = dialog.options.clone();
        options.header = dialog.header_input.read(cx).value().to_string();
        options.footer = dialog.footer_input.read(cx).value().to_string();
        let notebook_id = dialog.notebook_id;

        // Mark busy and return quickly so the frame can paint before we clone
        // markdown / write request.json / spawn the CLI.
        if let Some(dialog) = self.export.as_mut() {
            dialog.busy = true;
            dialog.confirm_overwrite = false;
            dialog.stage = Some("read".to_string());
            dialog.progress = None;
            dialog.warnings.clear();
            dialog.error = None;
            let display = output.display().to_string();
            dialog
                .output_input
                .update(cx, |state, cx| state.set_value(display, window, cx));
        }
        cx.notify();

        if let Some(id) = notebook_id {
            let prefs_export = options.clone();
            let open_after_export = self
                .export
                .as_ref()
                .map(|dialog| dialog.open_after_export)
                .unwrap_or(true);
            let parent = output.parent().map(Path::to_path_buf);
            let is_pdf = format == ExportFormat::Pdf;
            disk::enqueue(move || {
                let mut prefs = NotebookPrefs::load(&id);
                prefs.export = prefs_export;
                prefs.open_after_export = open_after_export;
                if let Some(parent) = parent {
                    prefs.set_export_dir(is_pdf, &parent);
                }
                if let Err(err) = prefs.save(&id) {
                    tracing::warn!(error = %err, "保存笔记本偏好失败");
                }
            });
        }

        cx.spawn(async move |this, cx| {
            let prepared = this
                .update(cx, |this, _cx| {
                    let dialog = this.export.as_ref()?;
                    if !dialog.busy {
                        return None;
                    }
                    Some(ExportRequest {
                        notebook_root: dialog.notebook_root.clone(),
                        note_rel: dialog.note_rel.clone(),
                        notebook_name: dialog.notebook_name.clone(),
                        title: dialog.title.clone(),
                        format,
                        output,
                        options,
                        markdown: Some(dialog.markdown.clone()),
                        temp_dir: None,
                    })
                })
                .ok()
                .flatten();
            let Some(request) = prepared else {
                return;
            };

            let result = cx
                .background_executor()
                .spawn(async move { ExportJob::start(&cli, &request) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(dialog) = this.export.as_mut() else {
                    if let Ok(job) = result {
                        job.cancel();
                    }
                    return;
                };
                if !dialog.busy {
                    if let Ok(job) = result {
                        job.cancel();
                    }
                    return;
                }
                match result {
                    Ok(job) => {
                        dialog.job = Some(job);
                        this.spawn_export_poll(cx);
                    }
                    Err(err) => {
                        dialog.busy = false;
                        dialog.job = None;
                        dialog.stage = None;
                        dialog.error = Some(err.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn spawn_export_poll(&self, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let keep = weak
                    .update(cx, |this, cx| this.poll_export(cx))
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn poll_export(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.export.as_mut() else {
            return false;
        };
        let Some(job) = dialog.job.as_ref() else {
            return false;
        };
        let events = job.drain();
        let finished = job.is_finished();
        let mut done = None;
        let mut failed = None;
        for event in events {
            match event {
                ExportEvent::Stage { stage } => dialog.stage = Some(stage),
                ExportEvent::Page { page, pages } => dialog.progress = Some((page, pages)),
                ExportEvent::Warning { message } => dialog.warnings.push(message),
                ExportEvent::Done { paths, pages } => done = Some((paths, pages)),
                ExportEvent::Error { message } => failed = Some(message),
            }
        }

        let mut close_with = None;
        let mut open_paths = Vec::new();
        if let Some(message) = failed {
            dialog.busy = false;
            dialog.job = None;
            dialog.error = Some(message);
        } else if let Some((paths, _pages)) = done {
            let should_open = dialog.open_after_export;
            dialog.busy = false;
            dialog.job = None;
            close_with = Some(if paths.len() == 1 {
                t!("export.done", path = paths[0].clone()).to_string()
            } else {
                t!("export.done_count", count = paths.len()).to_string()
            });
            if should_open {
                open_paths = paths;
            }
        } else if finished && dialog.job.is_some() {
            dialog.busy = false;
            dialog.job = None;
            dialog.error = Some(t!("export.failed").to_string());
        }

        let busy = dialog.job.is_some();
        if let Some(notice) = close_with {
            self.export = None;
            if let Some(path) = open_paths.first() {
                self.open_path_with_default_app(PathBuf::from(path), cx);
            }
            self.notify_success(notice, cx);
            return false;
        }
        busy
    }

    pub(super) fn render_export_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.export.as_ref() else {
            return div().into_any_element();
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let overlay = cx.theme().overlay;
        let danger = cx.theme().danger;
        // Name column is 64px, and each labeled row uses `gap_2` (8px), so
        // unlabeled controls start at 72px and stay in the value column.
        let label_width = px(64.);
        let field_indent = px(72.);
        let label =
            move |text: String| div().w(label_width).text_xs().text_color(muted).child(text);
        let format_button = |format: ExportFormat,
                             label_key: &'static str,
                             recommended: bool,
                             cx: &mut Context<Self>| {
            let active = dialog.format == format;
            let label = if recommended {
                format!("{} {}", t!(label_key), t!("export.recommended"))
            } else {
                t!(label_key).to_string()
            };
            Button::new(format!("export-format-{}", format.extension()))
                .ghost()
                .compact()
                .label(label)
                .when(recommended, |button| {
                    button.tooltip(t!("export.webp_hint").to_string())
                })
                .when(active, |button| button.primary())
                .disabled(dialog.busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_export_format(format, window, cx)
                }))
        };
        let checkbox = |id: &'static str,
                        label_key: &'static str,
                        checked: bool,
                        apply: fn(&mut ExportOptions, bool),
                        cx: &mut Context<Self>| {
            Checkbox::new(id)
                .label(t!(label_key).to_string())
                .checked(checked)
                .disabled(dialog.busy)
                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                    this.set_export_option(|options| apply(options, *checked), cx)
                }))
        };
        let shell = cx.entity();
        let option_menu = |id: &'static str,
                           text: String,
                           items: Vec<(String, bool, fn(&mut ExportOptions))>,
                           shell: Entity<Self>,
                           disabled: bool| {
            Button::new(id)
                .ghost()
                .compact()
                .label(text)
                .dropdown_caret(true)
                .disabled(disabled)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (label, selected, apply) in &items {
                        let shell = shell.clone();
                        let apply = *apply;
                        menu = menu.item(
                            PopupMenuItem::new(label.clone())
                                .checked(*selected)
                                .on_click(move |_, _, cx| {
                                    shell.update(cx, |this, cx| {
                                        this.set_export_option(apply, cx);
                                    });
                                }),
                        );
                    }
                    menu
                })
        };

        let paper_label = match dialog.options.paper {
            PaperSize::A4 => "A4",
            PaperSize::A5 => "A5",
            PaperSize::Letter => "Letter",
            PaperSize::Custom => "Custom",
        };
        let dpi = dialog.options.png_dpi;
        let margin_mm = dialog.options.margin_top_mm as i32;
        let font_pt = dialog.options.font_size_pt as i32;
        let line_height = (dialog.options.line_height * 100.0).round() as i32;
        let toc_depth = dialog.options.toc_depth;
        let image_pct = dialog.options.image_max_width_pct;
        let page_format = dialog.options.page_number_format.clone();
        let status = if let Some(error) = &dialog.error {
            Some((error.clone(), danger))
        } else if dialog.busy {
            let stage = dialog
                .stage
                .as_deref()
                .map(stage_label)
                .unwrap_or_else(|| t!("export.busy").to_string());
            let text = match dialog.progress {
                Some((page, pages)) => {
                    format!(
                        "{stage} · {}",
                        t!("export.page", page = page, pages = pages)
                    )
                }
                None => stage,
            };
            Some((text, muted))
        } else if dialog.confirm_overwrite {
            Some((t!("export.overwrite_warning").to_string(), danger))
        } else if !dialog.warnings.is_empty() {
            Some((
                t!("export.warnings", count = dialog.warnings.len()).to_string(),
                muted,
            ))
        } else {
            None
        };

        let output_value = dialog.output_input.read(cx).value().to_string();
        let can_start = !dialog.busy && !output_value.trim().is_empty();
        let start_label = if dialog.confirm_overwrite {
            t!("export.confirm_overwrite").to_string()
        } else {
            t!("export.start").to_string()
        };
        let is_raster = dialog.format.is_raster();

        div()
            .id("export-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .occlude()
            .child(
                div()
                    .id("export-dialog")
                    .v_flex()
                    // 832 leaves a 72px name column and still fits the shared toggles
                    // plus the image-only controls on one line. Those controls
                    // stay in the layout for every format, so the dialog does
                    // not resize when the format changes.
                    .w(px(832.))
                    .gap_3()
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_base()
                                    .font_bold()
                                    .child(t!("export.title").to_string()),
                            )
                            .child(
                                Button::new("export-close")
                                    .ghost()
                                    .compact()
                                    .label("×")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.close_export_dialog(cx)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(label(t!("export.format").to_string()))
                            .child(format_button(ExportFormat::Pdf, "export.pdf", false, cx))
                            .child(format_button(ExportFormat::Webp, "export.webp", true, cx))
                            .child(format_button(ExportFormat::Png, "export.png", false, cx))
                            .child(div().flex_1())
                            .child(label(t!("export.paper").to_string()))
                            .child(option_menu(
                                "export-paper",
                                paper_label.to_string(),
                                vec![
                                    (
                                        "A4".into(),
                                        dialog.options.paper == PaperSize::A4,
                                        |options| options.paper = PaperSize::A4,
                                    ),
                                    (
                                        "A5".into(),
                                        dialog.options.paper == PaperSize::A5,
                                        |options| options.paper = PaperSize::A5,
                                    ),
                                    (
                                        "Letter".into(),
                                        dialog.options.paper == PaperSize::Letter,
                                        |options| options.paper = PaperSize::Letter,
                                    ),
                                ],
                                shell.clone(),
                                dialog.busy,
                            )),
                    )
                    .child(
                        div()
                            .pl(field_indent)
                            .v_flex()
                            .gap_3()
                            .child(
                                div()
                                    .h_flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_3()
                                    .child(checkbox(
                                        "export-cover",
                                        "export.cover",
                                        dialog.options.cover,
                                        |options, value| options.cover = value,
                                        cx,
                                    ))
                                    .child(checkbox(
                                        "export-toc",
                                        "export.toc",
                                        dialog.options.toc,
                                        |options, value| options.toc = value,
                                        cx,
                                    ))
                                    .child(checkbox(
                                        "export-page-numbers",
                                        "export.page_numbers",
                                        dialog.options.page_numbers,
                                        |options, value| options.page_numbers = value,
                                        cx,
                                    ))
                                    .child(checkbox(
                                        "export-code-wrap",
                                        "export.code_wrap",
                                        dialog.options.code_wrap,
                                        |options, value| options.code_wrap = value,
                                        cx,
                                    ))
                                    .child(checkbox(
                                        "export-code-highlight",
                                        "export.code_highlight",
                                        dialog.options.code_highlight,
                                        |options, value| options.code_highlight = value,
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .h_flex()
                                            .flex_shrink_0()
                                            .items_center()
                                            .gap_3()
                                            .when(!is_raster, |group| group.invisible())
                                            .child(option_menu(
                                                "export-dpi",
                                                t!("export.dpi", dpi = dpi).to_string(),
                                                vec![
                                                    (
                                                        t!("export.dpi", dpi = 96).to_string(),
                                                        dpi == 96,
                                                        |options| options.png_dpi = 96,
                                                    ),
                                                    (
                                                        t!("export.dpi", dpi = 144).to_string(),
                                                        dpi == 144,
                                                        |options| options.png_dpi = 144,
                                                    ),
                                                    (
                                                        t!("export.dpi", dpi = 192).to_string(),
                                                        dpi == 192,
                                                        |options| options.png_dpi = 192,
                                                    ),
                                                    (
                                                        t!("export.dpi", dpi = 300).to_string(),
                                                        dpi == 300,
                                                        |options| options.png_dpi = 300,
                                                    ),
                                                ],
                                                shell.clone(),
                                                dialog.busy || !is_raster,
                                            ))
                                            .child(
                                                checkbox(
                                                    "export-page-files",
                                                    "export.page_files",
                                                    dialog.options.png_page_files,
                                                    |options, value| options.png_page_files = value,
                                                    cx,
                                                )
                                                .disabled(dialog.busy || !is_raster),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(option_menu(
                                        "export-margin",
                                        t!("export.margin", mm = margin_mm).to_string(),
                                        vec![
                                            (
                                                t!("export.margin", mm = 10).to_string(),
                                                margin_mm == 10,
                                                |options| set_uniform_margin(options, 10.0),
                                            ),
                                            (
                                                t!("export.margin", mm = 15).to_string(),
                                                margin_mm == 15,
                                                |options| set_uniform_margin(options, 15.0),
                                            ),
                                            (
                                                t!("export.margin", mm = 20).to_string(),
                                                margin_mm == 20,
                                                |options| set_uniform_margin(options, 20.0),
                                            ),
                                            (
                                                t!("export.margin", mm = 25).to_string(),
                                                margin_mm == 25,
                                                |options| set_uniform_margin(options, 25.0),
                                            ),
                                            (
                                                t!("export.margin", mm = 30).to_string(),
                                                margin_mm == 30,
                                                |options| set_uniform_margin(options, 30.0),
                                            ),
                                        ],
                                        shell.clone(),
                                        dialog.busy,
                                    ))
                                    .child(option_menu(
                                        "export-font-size",
                                        t!(
                                            "export.font_size",
                                            pt = format_pt(dialog.options.font_size_pt)
                                        )
                                        .to_string(),
                                        vec![
                                            (
                                                t!("export.font_size", pt = "9").to_string(),
                                                font_pt == 9,
                                                |options| options.font_size_pt = 9.0,
                                            ),
                                            (
                                                t!("export.font_size", pt = "10").to_string(),
                                                font_pt == 10,
                                                |options| options.font_size_pt = 10.0,
                                            ),
                                            (
                                                t!("export.font_size", pt = "11").to_string(),
                                                font_pt == 11,
                                                |options| options.font_size_pt = 11.0,
                                            ),
                                            (
                                                t!("export.font_size", pt = "12").to_string(),
                                                font_pt == 12,
                                                |options| options.font_size_pt = 12.0,
                                            ),
                                            (
                                                t!("export.font_size", pt = "13").to_string(),
                                                font_pt == 13,
                                                |options| options.font_size_pt = 13.0,
                                            ),
                                            (
                                                t!("export.font_size", pt = "14").to_string(),
                                                font_pt == 14,
                                                |options| options.font_size_pt = 14.0,
                                            ),
                                        ],
                                        shell.clone(),
                                        dialog.busy,
                                    ))
                                    .child(option_menu(
                                        "export-line-height",
                                        t!(
                                            "export.line_height",
                                            value = format!("{:.2}", dialog.options.line_height)
                                        )
                                        .to_string(),
                                        vec![
                                            (
                                                t!("export.line_height", value = "1.40")
                                                    .to_string(),
                                                line_height == 140,
                                                |options| options.line_height = 1.4,
                                            ),
                                            (
                                                t!("export.line_height", value = "1.50")
                                                    .to_string(),
                                                line_height == 150,
                                                |options| options.line_height = 1.5,
                                            ),
                                            (
                                                t!("export.line_height", value = "1.65")
                                                    .to_string(),
                                                line_height == 165,
                                                |options| options.line_height = 1.65,
                                            ),
                                            (
                                                t!("export.line_height", value = "1.80")
                                                    .to_string(),
                                                line_height == 180,
                                                |options| options.line_height = 1.8,
                                            ),
                                            (
                                                t!("export.line_height", value = "2.00")
                                                    .to_string(),
                                                line_height == 200,
                                                |options| options.line_height = 2.0,
                                            ),
                                        ],
                                        shell.clone(),
                                        dialog.busy,
                                    ))
                                    .child(option_menu(
                                        "export-toc-depth",
                                        t!("export.toc_depth", depth = toc_depth).to_string(),
                                        vec![
                                            (
                                                t!("export.toc_depth", depth = 1).to_string(),
                                                toc_depth == 1,
                                                |options| options.toc_depth = 1,
                                            ),
                                            (
                                                t!("export.toc_depth", depth = 2).to_string(),
                                                toc_depth == 2,
                                                |options| options.toc_depth = 2,
                                            ),
                                            (
                                                t!("export.toc_depth", depth = 3).to_string(),
                                                toc_depth == 3,
                                                |options| options.toc_depth = 3,
                                            ),
                                            (
                                                t!("export.toc_depth", depth = 4).to_string(),
                                                toc_depth == 4,
                                                |options| options.toc_depth = 4,
                                            ),
                                            (
                                                t!("export.toc_depth", depth = 5).to_string(),
                                                toc_depth == 5,
                                                |options| options.toc_depth = 5,
                                            ),
                                        ],
                                        shell.clone(),
                                        dialog.busy,
                                    ))
                                    .child(option_menu(
                                        "export-table-width",
                                        match dialog.options.table_width {
                                            TableWidth::Content => {
                                                t!("export.table_content").to_string()
                                            }
                                            TableWidth::Full => t!("export.table_full").to_string(),
                                        },
                                        vec![
                                            (
                                                t!("export.table_content").to_string(),
                                                dialog.options.table_width == TableWidth::Content,
                                                |options| options.table_width = TableWidth::Content,
                                            ),
                                            (
                                                t!("export.table_full").to_string(),
                                                dialog.options.table_width == TableWidth::Full,
                                                |options| options.table_width = TableWidth::Full,
                                            ),
                                        ],
                                        shell.clone(),
                                        dialog.busy,
                                    ))
                                    .child(option_menu(
                                        "export-image-width",
                                        t!("export.image_width", pct = image_pct).to_string(),
                                        vec![
                                            (
                                                t!("export.image_width", pct = 60).to_string(),
                                                image_pct == 60,
                                                |options| options.image_max_width_pct = 60,
                                            ),
                                            (
                                                t!("export.image_width", pct = 75).to_string(),
                                                image_pct == 75,
                                                |options| options.image_max_width_pct = 75,
                                            ),
                                            (
                                                t!("export.image_width", pct = 90).to_string(),
                                                image_pct == 90,
                                                |options| options.image_max_width_pct = 90,
                                            ),
                                            (
                                                t!("export.image_width", pct = 100).to_string(),
                                                image_pct == 100,
                                                |options| options.image_max_width_pct = 100,
                                            ),
                                        ],
                                        shell.clone(),
                                        dialog.busy,
                                    ))
                                    .child(option_menu(
                                        "export-page-format",
                                        format!("{}: {page_format}", t!("export.page_format")),
                                        vec![
                                            (
                                                format!("{}: 1", t!("export.page_format")),
                                                page_format == "1",
                                                |options| {
                                                    options.page_number_format = "1".to_string()
                                                },
                                            ),
                                            (
                                                format!("{}: 1 / 1", t!("export.page_format")),
                                                page_format == "1 / 1",
                                                |options| {
                                                    options.page_number_format = "1 / 1".to_string()
                                                },
                                            ),
                                            (
                                                format!("{}: - 1 -", t!("export.page_format")),
                                                page_format == "- 1 -",
                                                |options| {
                                                    options.page_number_format = "- 1 -".to_string()
                                                },
                                            ),
                                            (
                                                format!("{}: 1 / 2", t!("export.page_format")),
                                                page_format == "1 / 2",
                                                |options| {
                                                    options.page_number_format = "1 / 2".to_string()
                                                },
                                            ),
                                        ],
                                        shell.clone(),
                                        dialog.busy,
                                    ))
                                    .child(checkbox(
                                        "export-justify",
                                        "export.justify",
                                        dialog.options.justify,
                                        |options, value| options.justify = value,
                                        cx,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(label(t!("export.header").to_string()))
                            .child(Input::new(&dialog.header_input).w_full().small()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(label(t!("export.footer").to_string()))
                            .child(Input::new(&dialog.footer_input).w_full().small()),
                    )
                    .child(
                        div()
                            .v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(label(t!("export.output").to_string()))
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&dialog.output_input).w_full().small(),
                                        ),
                                    )
                                    .child(
                                        Button::new("export-browse")
                                            .ghost()
                                            .compact()
                                            .label(t!("export.browse").to_string())
                                            .disabled(dialog.busy)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.pick_export_output(window, cx)
                                            })),
                                    ),
                            )
                            .child(
                                div().pl(field_indent).child(
                                    Checkbox::new("export-open-after")
                                        .label(t!("export.open_after").to_string())
                                        .checked(dialog.open_after_export)
                                        .disabled(dialog.busy)
                                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                            if let Some(dialog) = this.export.as_mut() {
                                                dialog.open_after_export = *checked;
                                                cx.notify();
                                            }
                                        })),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .min_h(px(24.))
                            // Keep「取消」away from the former「开始导出」hit target so a
                            // double-click cannot cancel. Occupy the start-button slot with a
                            // disabled primary while busy.
                            .when(dialog.busy, |row| {
                                row.child(
                                    Button::new("export-cancel")
                                        .ghost()
                                        .compact()
                                        .label(t!("export.cancel").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_export_dialog(cx)
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .when_some(status, |element, (text, color)| {
                                        element.text_color(color).child(text)
                                    }),
                            )
                            .when(dialog.busy, |row| {
                                row.child(
                                    Button::new("export-busy")
                                        .primary()
                                        .compact()
                                        .label(t!("export.busy_button").to_string())
                                        .disabled(true),
                                )
                            })
                            .when(!dialog.busy, |row| {
                                row.child(
                                    Button::new("export-dismiss")
                                        .ghost()
                                        .compact()
                                        .label(t!("export.close").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_export_dialog(cx)
                                        })),
                                )
                                .child(
                                    Button::new("export-start")
                                        .primary()
                                        .compact()
                                        .label(start_label)
                                        .disabled(!can_start)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.begin_export(window, cx)
                                        })),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}

fn set_uniform_margin(options: &mut ExportOptions, mm: f32) {
    options.margin_top_mm = mm;
    options.margin_bottom_mm = mm;
    options.margin_left_mm = mm;
    options.margin_right_mm = mm;
}

fn export_target_exists(output: &Path) -> bool {
    if output.exists() {
        return true;
    }
    let Some(parent) = output.parent() else {
        return false;
    };
    let Some(stem) = output.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    let Some(extension) = output.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return false;
    };
    let prefix = format!("{stem}-");
    let extension_lower = extension.to_ascii_lowercase();
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return false;
        };
        let Some((base, ext)) = name.rsplit_once('.') else {
            return false;
        };
        base.starts_with(&prefix) && ext.eq_ignore_ascii_case(&extension_lower)
    })
}
