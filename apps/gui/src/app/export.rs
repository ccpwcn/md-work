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
            let (prefs, dir) = cx
                .background_executor()
                .spawn(async move {
                    let prefs = notebook_id
                        .map(|id| NotebookPrefs::load(&id))
                        .unwrap_or_default();
                    let dir = prefs.export_dir(true).unwrap_or(fallback);
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
            header_input,
            footer_input,
            output_input,
            job: None,
            busy: false,
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
        let output = ensure_export_extension(raw_output, dialog.format);
        let mut options = dialog.options.clone();
        options.header = dialog.header_input.read(cx).value().to_string();
        options.footer = dialog.footer_input.read(cx).value().to_string();
        let request = ExportRequest {
            notebook_root: dialog.notebook_root.clone(),
            note_rel: dialog.note_rel.clone(),
            notebook_name: dialog.notebook_name.clone(),
            title: dialog.title.clone(),
            format: dialog.format,
            output: output.clone(),
            options: options.clone(),
            markdown: Some(dialog.markdown.clone()),
            temp_dir: None,
        };
        let notebook_id = dialog.notebook_id;

        if let Some(id) = notebook_id {
            let prefs_export = options.clone();
            let parent = output.parent().map(Path::to_path_buf);
            let is_pdf = dialog.format == ExportFormat::Pdf;
            disk::enqueue(move || {
                let mut prefs = NotebookPrefs::load(&id);
                prefs.export = prefs_export;
                if let Some(parent) = parent {
                    prefs.set_export_dir(is_pdf, &parent);
                }
                if let Err(err) = prefs.save(&id) {
                    tracing::warn!(error = %err, "保存笔记本偏好失败");
                }
            });
        }

        match ExportJob::start(&cli, &request) {
            Ok(job) => {
                if let Some(dialog) = self.export.as_mut() {
                    dialog.job = Some(job);
                    dialog.busy = true;
                    dialog.stage = Some("read".to_string());
                    dialog.progress = None;
                    dialog.warnings.clear();
                    dialog.error = None;
                    let display = output.display().to_string();
                    dialog
                        .output_input
                        .update(cx, |state, cx| state.set_value(display, window, cx));
                }
                self.spawn_export_poll(cx);
            }
            Err(err) => {
                if let Some(dialog) = self.export.as_mut() {
                    dialog.error = Some(err.to_string());
                }
            }
        }
        cx.notify();
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
        if let Some(message) = failed {
            dialog.busy = false;
            dialog.job = None;
            dialog.error = Some(message);
        } else if let Some((paths, _pages)) = done {
            dialog.busy = false;
            dialog.job = None;
            close_with = Some(if paths.len() == 1 {
                t!("export.done", path = paths[0].clone()).to_string()
            } else {
                t!("export.done_count", count = paths.len()).to_string()
            });
        } else if finished && dialog.job.is_some() {
            dialog.busy = false;
            dialog.job = None;
            dialog.error = Some(t!("export.failed").to_string());
        }

        let busy = dialog.job.is_some();
        if let Some(notice) = close_with {
            self.export = None;
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
        let label = |text: String| div().w(px(64.)).text_xs().text_color(muted).child(text);
        let format_button =
            |format: ExportFormat, label_key: &'static str, cx: &mut Context<Self>| {
                let active = dialog.format == format;
                Button::new(format!("export-format-{}", format.extension()))
                    .ghost()
                    .compact()
                    .label(t!(label_key).to_string())
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
        let cycle = |id: &'static str,
                     text: String,
                     apply: fn(&mut ExportOptions),
                     cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .compact()
                .label(text)
                .disabled(dialog.busy)
                .on_click(cx.listener(move |this, _, _, cx| this.set_export_option(apply, cx)))
        };

        let paper_label = match dialog.options.paper {
            PaperSize::A4 => "A4",
            PaperSize::A5 => "A5",
            PaperSize::Letter => "Letter",
            PaperSize::Custom => "Custom",
        };
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
                    .w(px(620.))
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
                            .child(format_button(ExportFormat::Pdf, "export.pdf", cx))
                            .child(format_button(ExportFormat::Png, "export.png", cx))
                            .child(div().flex_1())
                            .child(label(t!("export.paper").to_string()))
                            .child(
                                Button::new("export-paper")
                                    .ghost()
                                    .compact()
                                    .label(paper_label.to_string())
                                    .disabled(dialog.busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_export_option(
                                            |options| {
                                                options.paper = match options.paper {
                                                    PaperSize::A4 => PaperSize::A5,
                                                    PaperSize::A5 => PaperSize::Letter,
                                                    PaperSize::Letter | PaperSize::Custom => {
                                                        PaperSize::A4
                                                    }
                                                }
                                            },
                                            cx,
                                        )
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_3()
                            .when(dialog.format == ExportFormat::Png, |row| {
                                row.child(
                                    Button::new("export-dpi")
                                        .ghost()
                                        .compact()
                                        .label(
                                            t!("export.dpi", dpi = dialog.options.png_dpi)
                                                .to_string(),
                                        )
                                        .disabled(dialog.busy)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set_export_option(
                                                |options| {
                                                    options.png_dpi = match options.png_dpi {
                                                        0..=120 => 144,
                                                        121..=168 => 192,
                                                        169..=240 => 300,
                                                        _ => 96,
                                                    }
                                                },
                                                cx,
                                            )
                                        })),
                                )
                            })
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
                            .when(dialog.format == ExportFormat::Png, |row| {
                                row.child(checkbox(
                                    "export-page-files",
                                    "export.page_files",
                                    dialog.options.png_page_files,
                                    |options, value| options.png_page_files = value,
                                    cx,
                                ))
                            }),
                    )
                    .child(
                        div()
                            .h_flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(cycle(
                                "export-margin",
                                t!("export.margin", mm = dialog.options.margin_top_mm as i32)
                                    .to_string(),
                                |options| {
                                    let next = match options.margin_top_mm as i32 {
                                        10 => 15.0,
                                        15 => 20.0,
                                        20 => 25.0,
                                        25 => 30.0,
                                        _ => 10.0,
                                    };
                                    options.margin_top_mm = next;
                                    options.margin_bottom_mm = next;
                                    options.margin_left_mm = next;
                                    options.margin_right_mm = next;
                                },
                                cx,
                            ))
                            .child(cycle(
                                "export-font-size",
                                t!(
                                    "export.font_size",
                                    pt = format_pt(dialog.options.font_size_pt)
                                )
                                .to_string(),
                                |options| {
                                    options.font_size_pt = match options.font_size_pt as i32 {
                                        9 => 10.0,
                                        10 => 11.0,
                                        11 => 12.0,
                                        12 => 13.0,
                                        13 => 14.0,
                                        _ => 9.0,
                                    };
                                },
                                cx,
                            ))
                            .child(cycle(
                                "export-line-height",
                                t!(
                                    "export.line_height",
                                    value = format!("{:.2}", dialog.options.line_height)
                                )
                                .to_string(),
                                |options| {
                                    options.line_height =
                                        match (options.line_height * 100.0).round() as i32 {
                                            140 => 1.5,
                                            150 => 1.65,
                                            165 => 1.8,
                                            180 => 2.0,
                                            _ => 1.4,
                                        };
                                },
                                cx,
                            ))
                            .child(cycle(
                                "export-toc-depth",
                                t!("export.toc_depth", depth = dialog.options.toc_depth)
                                    .to_string(),
                                |options| {
                                    options.toc_depth = match options.toc_depth {
                                        1 => 2,
                                        2 => 3,
                                        3 => 4,
                                        4 => 5,
                                        _ => 1,
                                    };
                                },
                                cx,
                            ))
                            .child(cycle(
                                "export-image-width",
                                t!(
                                    "export.image_width",
                                    pct = dialog.options.image_max_width_pct
                                )
                                .to_string(),
                                |options| {
                                    options.image_max_width_pct = match options.image_max_width_pct
                                    {
                                        60 => 75,
                                        61..=75 => 90,
                                        76..=90 => 100,
                                        _ => 60,
                                    };
                                },
                                cx,
                            ))
                            .child(cycle(
                                "export-page-format",
                                format!(
                                    "{}: {}",
                                    t!("export.page_format"),
                                    dialog.options.page_number_format
                                ),
                                |options| {
                                    options.page_number_format =
                                        match options.page_number_format.as_str() {
                                            "1" => "1 / 1".to_string(),
                                            "1 / 1" => "- 1 -".to_string(),
                                            "- 1 -" => "1 / 2".to_string(),
                                            _ => "1".to_string(),
                                        };
                                },
                                cx,
                            ))
                            .child(checkbox(
                                "export-justify",
                                "export.justify",
                                dialog.options.justify,
                                |options, value| options.justify = value,
                                cx,
                            )),
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
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(label(t!("export.output").to_string()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&dialog.output_input).w_full().small()),
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
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .min_h(px(24.))
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
                                    Button::new("export-cancel")
                                        .ghost()
                                        .compact()
                                        .label(t!("export.cancel").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_export_dialog(cx)
                                        })),
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
                                        .label(t!("export.start").to_string())
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
