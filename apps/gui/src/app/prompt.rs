use super::*;

impl AppShell {
    pub(super) fn open_external_notebook_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(String::new())
                .placeholder(t!("prompt.open_notebook_placeholder").to_string())
                .clean_on_escape()
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, move |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_prompt(cx);
            }
        })
        .detach();
        self.prompt = Some(PromptDialog {
            kind: PromptKind::OpenNotebook,
            title: t!("prompt.open_notebook").to_string(),
            hint: t!("prompt.open_notebook_hint").to_string(),
            input,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn close_prompt(&mut self, cx: &mut Context<Self>) {
        if self.prompt.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn close_recovery(&mut self, cx: &mut Context<Self>) {
        if self.recovery.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn restore_recovery(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self
            .recovery
            .as_ref()
            .and_then(|dialog| dialog.entries.get(index).cloned())
        else {
            return;
        };
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let path = snapshot.note_path.clone();
            let exists = cx
                .background_executor()
                .spawn(async move { path.is_file() })
                .await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.apply_recovery_restore(snapshot, exists, window, cx)
                });
            });
        })
        .detach();
    }

    pub(super) fn apply_recovery_restore(
        &mut self,
        snapshot: mdwork_core::recovery::Snapshot,
        exists: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !exists {
            self.notify_warning(t!("recovery.missing").to_string(), cx);
            return;
        }
        let already_open = self
            .docs
            .iter()
            .find(|doc| doc.read(cx).path == snapshot.note_path)
            .cloned();
        let Some(doc) = already_open else {
            let path = snapshot.note_path.to_string_lossy().into_owned();
            self.pending_recovery = Some(snapshot);
            self.open_path(&path, window, cx);
            return;
        };
        let disk_changed = snapshot
            .base_sha256
            .as_deref()
            .is_some_and(|base| base != doc.read(cx).base_hash.as_str());
        let text = snapshot.text.clone();
        doc.update(cx, |doc, cx| doc.replace_all(text, window, cx));
        let note_path = snapshot.note_path.clone();
        disk::enqueue(move || {
            let _ = mdwork_core::recovery::remove(&mdwork_core::paths::recovery_dir(), &note_path);
        });
        self.remove_recovery_entry(&snapshot.note_path);
        let notice = if disk_changed {
            t!("recovery.disk_changed").to_string()
        } else {
            t!("recovery.restored").to_string()
        };
        self.notify_success(notice, cx);
        cx.notify();
    }

    pub(super) fn remove_recovery_entry(&mut self, note_path: &Path) {
        let mut empty = false;
        if let Some(dialog) = self.recovery.as_mut() {
            dialog.entries.retain(|entry| entry.note_path != note_path);
            empty = dialog.entries.is_empty();
        }
        if empty {
            self.recovery = None;
        }
    }

    pub(super) fn discard_recovery(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(dialog) = self.recovery.as_mut() else {
            return;
        };
        let Some(snapshot) = dialog.entries.get(index).cloned() else {
            return;
        };
        let note_path = snapshot.note_path.clone();
        disk::enqueue(move || {
            let _ = mdwork_core::recovery::remove(&mdwork_core::paths::recovery_dir(), &note_path);
        });
        self.remove_recovery_entry(&snapshot.note_path);
        cx.notify();
    }

    pub(super) fn discard_all_recovery(&mut self, cx: &mut Context<Self>) {
        disk::enqueue(|| {
            let _ = mdwork_core::recovery::clear(&mdwork_core::paths::recovery_dir());
        });
        self.recovery = None;
        cx.notify();
    }

    pub(super) fn submit_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let value = prompt.input.read(cx).value().trim().to_string();
        if value.is_empty() {
            self.prompt = Some(PromptDialog {
                error: Some(t!("prompt.empty").to_string()),
                ..prompt
            });
            cx.notify();
            return;
        }
        match prompt.kind.clone() {
            PromptKind::MoveEntry { notebook_id, from } => {
                self.begin_move_entry(notebook_id, from, value, cx);
            }
            PromptKind::OpenNotebook => {
                let path = PathBuf::from(&value);
                let Some(workspace) = self.workspace.clone() else {
                    cx.notify();
                    return;
                };
                let receiver = disk::submit(move || workspace.register_external(&path));
                cx.spawn(async move |this, cx| {
                    let result = match receiver.await {
                        Ok(result) => result,
                        Err(_) => return,
                    };
                    let _ = this.update(cx, |this, cx| {
                        match result {
                            Ok(notebook) => {
                                this.active_notebook = Some(notebook.marker.id);
                                this.reload(cx);
                                this.persist_session(cx);
                                this.reveal_active(cx);
                            }
                            Err(err) => this.notify_error(format!("打开笔记本失败：{err}"), cx),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    pub(super) fn save_as_encoding(
        &mut self,
        encoding: TextEncoding,
        bom: bool,
        eol: Eol,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            self.notify_warning(t!("encoding.no_doc").to_string(), cx);
            return;
        };
        doc.update(cx, |doc, cx| doc.save_as_encoding(encoding, bom, eol, cx));
        self.notify_success(
            t!(
                "encoding.saved_as",
                encoding = encoding.label(),
                eol = eol.label()
            )
            .to_string(),
            cx,
        );
        cx.notify();
    }

    pub(super) fn confirm_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirm) = self.workspace_confirm.take() else {
            return;
        };
        self.switch_workspace(confirm.path, window, cx);
    }

    pub(super) fn cancel_workspace_confirm(&mut self, cx: &mut Context<Self>) {
        if self.workspace_confirm.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn apply_workspace_name(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.current_workspace_root() else {
            return;
        };
        let Some(dialog) = self.settings.as_ref() else {
            return;
        };
        let name = dialog.workspace_name.read(cx).value().trim().to_string();
        if !self.workspace_registry.set_name(&root, &name) {
            return;
        }
        self.save_registry(cx);
        self.notify_success(t!("workspace.name_saved").to_string(), cx);
        cx.notify();
    }
}
