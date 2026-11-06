use super::*;

impl AppShell {
    pub(super) fn refresh_outline(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            self.outline = Arc::default();
            self.outline_cache = None;
            return;
        };
        let (id, revision) = {
            let doc = doc.read(cx);
            (doc.editor.entity_id(), doc.revision())
        };
        if self.outline_cache == Some((id, revision)) {
            return;
        }
        let ir = match &self.preview_cache {
            Some(cache) if cache.doc == id && cache.revision == revision => cache.ir.clone(),
            _ => {
                let text = doc.read(cx).text(cx);
                PreviewIr::from_markdown(&text)
            }
        };
        self.outline = Arc::new(ir.outline());
        self.outline_cache = Some((id, revision));
    }

    pub(super) fn set_search_scope(&mut self, all: bool, cx: &mut Context<Self>) {
        if self.search_all == all {
            return;
        }
        self.search_all = all;
        self.schedule_search(cx);
    }

    pub(super) fn toggle_search_option(&mut self, option: SearchOption, cx: &mut Context<Self>) {
        match option {
            SearchOption::Case => self.search_case = !self.search_case,
            SearchOption::Regex => self.search_regex = !self.search_regex,
            SearchOption::Filename => self.search_filename = !self.search_filename,
        }
        self.schedule_search(cx);
    }

    pub(super) fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation += 1;
        let generation = self.search_generation;
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.search_cancel.clone();

        let mut query = SearchQuery::new(self.search_input.read(cx).value().to_string());
        query.case_sensitive = self.search_case;
        query.regex = self.search_regex;
        query.filename = self.search_filename;
        if query.is_blank() {
            self.search_results.clear();
            self.search_status = SearchStatus::Idle;
            cx.notify();
            return;
        }

        let notebooks: Vec<Notebook> = if self.search_all {
            self.notebooks
                .iter()
                .map(|entry| entry.notebook.clone())
                .collect()
        } else {
            self.active_notebook
                .and_then(|id| {
                    self.notebooks
                        .iter()
                        .find(|entry| entry.notebook.marker.id == id)
                })
                .map(|entry| entry.notebook.clone())
                .into_iter()
                .collect()
        };
        self.search_status = SearchStatus::Running;
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(220))
                .await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let mut hits = Vec::new();
                    let mut error = None;
                    for notebook in &notebooks {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        match search::search_notebook(
                            notebook,
                            &query,
                            &cancel,
                            search::DEFAULT_MAX_HITS,
                        ) {
                            Ok(result) => {
                                let stop = result.cancelled;
                                hits.extend(result.hits);
                                if stop {
                                    break;
                                }
                            }
                            Err(err) => {
                                error = Some(err.to_string());
                                break;
                            }
                        }
                    }
                    (hits, error)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                match outcome {
                    (_, Some(err)) => {
                        this.search_results.clear();
                        this.search_status = SearchStatus::Failed(err);
                    }
                    (hits, None) => {
                        this.search_results = hits;
                        this.search_status = SearchStatus::Idle;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
