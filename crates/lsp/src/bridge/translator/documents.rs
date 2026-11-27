//! The documents a server is sent and kept in step with the disk, and what
//! each server can be asked.

use super::*;

impl Translator {
    /// Forget which documents were sent to `language_id`'s server, and which
    /// changes on disk it heard of, so that the next request for each
    /// document sends it to the server running then. Remembered across a
    /// restart, they would never reach the new server, which would fail every
    /// request for them.
    pub(super) async fn forget_documents(&self, language_id: &str) {
        self.open_documents
            .write()
            .await
            .retain(|path, _| self.get_language_id(Path::new(path)) != language_id);
        self.disk_changes.lock().stopped(language_id);
    }

    /// Bring `language_id`'s server up to date with the disk before a
    /// request: send the documents it holds open that changed on disk again,
    /// close those deleted, and tell it of the files created, changed or
    /// deleted under its folders since it last heard
    /// (`workspace/didChangeWatchedFiles`). A server holding a document open
    /// answers from the text it was sent, and one that was not told of a
    /// change from the version it read, so either would answer about code
    /// that is gone. Returns whether the server was told anything.
    pub(super) async fn sync_with_disk(
        &self,
        client: &LspClient,
        language_id: &str,
    ) -> Result<bool> {
        let open: Vec<String> = self
            .open_documents
            .read()
            .await
            .keys()
            .filter(|path| self.get_language_id(Path::new(path)) == language_id)
            .cloned()
            .collect();
        let mut told = false;
        for path in &open {
            if disk_stamp(Path::new(path)).await.is_none() {
                told |= self.close_document(client, path).await;
            } else {
                told |= self.send_document(client, Path::new(path), None).await?;
            }
        }

        let folders = self
            .server_folders
            .read()
            .await
            .get(language_id)
            .cloned()
            .unwrap_or_default();
        let pending = self.disk_changes.lock().take(language_id, |path| {
            folders
                .iter()
                .any(|folder| Path::new(path).starts_with(folder))
        });
        let still_open = self.open_documents.read().await;
        // A document held open is the client's to tell of: sent above.
        let changes: Vec<lsp_types::FileEvent> = pending
            .into_iter()
            .filter(|(path, _)| !still_open.contains_key(path))
            .filter_map(|(path, change)| {
                Some(lsp_types::FileEvent {
                    uri: path_to_uri(Path::new(&path)).ok()?,
                    typ: change.file_change_type(),
                })
            })
            .collect();
        drop(still_open);
        if !changes.is_empty() {
            client
                .notify(
                    "workspace/didChangeWatchedFiles",
                    lsp_types::DidChangeWatchedFilesParams { changes },
                )
                .await
                .map_err(|e| LspError::Io(format!("didChangeWatchedFiles failed: {e}")))?;
            told = true;
        }
        Ok(told)
    }

    /// [`Self::sync_with_disk`] before a request that names no file, then let
    /// the server digest what it was told.
    pub(super) async fn sync_for_request(
        &self,
        client: &LspClient,
        language_id: &str,
    ) -> Result<()> {
        if self.sync_with_disk(client, language_id).await? {
            client.wait_until_ready().await;
        }
        Ok(())
    }

    /// Close the open document at `key` in `client`'s server: its file is
    /// gone. Returns whether it was open.
    pub(super) async fn close_document(&self, client: &LspClient, key: &str) -> bool {
        if self.open_documents.write().await.remove(key).is_none() {
            return false;
        }
        if let Ok(uri) = path_to_uri(Path::new(key)) {
            let params = lsp_types::DidCloseTextDocumentParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
            };
            if let Err(e) = client.notify("textDocument/didClose", params).await {
                warn!("didClose for {key} failed: {e}");
            }
        }
        true
    }

    /// Make the running `language_id` server cover `file_path`'s project.
    ///
    /// A server that knows only the projects among its workspace folders
    /// ([`Self::knows_only_its_folders`]) is restarted with the file's project
    /// added to them: rust-analyzer started for one crate of a monorepo
    /// answered `file not found` for every file of another. Returns the new
    /// client, or `None` when the running server covers the file already or is
    /// of another kind.
    pub(super) async fn cover_project(
        &self,
        language_id: &str,
        file_path: &Path,
    ) -> Result<Option<LspClient>> {
        if !Self::knows_only_its_folders(language_id) {
            return Ok(None);
        }
        let Some(config) = self.server_config(language_id) else {
            return Ok(None);
        };
        let Some(project) = Self::workspace_root_for(&config, file_path).map(|p| canonical(&p))
        else {
            return Ok(None);
        };
        let covers = |folders: &[PathBuf]| folders.iter().any(|f| project.starts_with(f));
        let folders = self.server_folders.read().await.get(language_id).cloned();
        if folders.as_deref().is_none_or(covers) {
            return Ok(None);
        }

        let _guard = self.lazy_spawn_lock.lock().await;
        // Another caller may have added it while we waited.
        let mut folders = self
            .server_folders
            .read()
            .await
            .get(language_id)
            .cloned()
            .unwrap_or_default();
        if covers(&folders) {
            return Ok(self.clients.get(language_id));
        }
        info!(
            "Restarting {language_id} server to add {} to {}",
            project.display(),
            show_folders(&folders)
        );
        folders.push(project);
        self.register_server(&config, folders).await.map(Some)
    }

    /// How many files [`Self::open_beside`] opens at most.
    const MAX_OPENED_BESIDE: usize = 64;

    /// The folders of the server that answers for `file`, started for it if
    /// none runs: where the files it can be asked about are.
    ///
    /// # Errors
    ///
    /// Returns an error if no server can answer for `file`.
    pub async fn folders_serving(&self, file: &Path) -> Result<Vec<PathBuf>> {
        self.get_client_for_file(file).await?;
        let language = self.get_language_id(file);
        Ok(self
            .server_folders
            .read()
            .await
            .get(&language)
            .cloned()
            .unwrap_or_default())
    }

    /// Open `files` in the server that answers for `file`, as a request about
    /// each would — those of its language, at most [`Self::MAX_OPENED_BESIDE`]
    /// — so that a server that looks for references only in the projects of
    /// open files finds theirs: typescript-language-server, with no
    /// `jsconfig.json`, gives each open file a project of its own and of what
    /// it imports, and found none of a test file's calls until the test file
    /// was open.
    ///
    /// # Errors
    ///
    /// Returns an error if no server can answer for `file`; a file that
    /// cannot be opened is passed over.
    pub async fn open_beside(&self, file: &Path, files: &[PathBuf]) -> Result<()> {
        let client = self.get_client_for_file(file).await?;
        let language = self.get_language_id(file);
        let mut sent = false;
        for other in files
            .iter()
            .filter(|other| other.as_path() != file && self.get_language_id(other) == language)
            .take(Self::MAX_OPENED_BESIDE)
        {
            match self.ensure_document_open(&client, other).await {
                Ok(opened) => sent |= opened,
                Err(e) => debug!("Not opening {}: {e}", other.display()),
            }
        }
        if sent {
            client.wait_until_ready().await;
        }
        Ok(())
    }

    /// Open `project`'s files of `language_id` in its server, as
    /// [`Self::open_beside`] opens a symbol's users: gopls loads a module,
    /// and typescript-language-server a project, only once a file of it is
    /// open, and before that searched none of its symbols.
    pub(super) async fn open_project(&self, language_id: &str, project: &Path) -> Result<()> {
        let files: Vec<PathBuf> = ignore::WalkBuilder::new(project)
            .build()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            .map(ignore::DirEntry::into_path)
            .filter(|path| self.get_language_id(path) == language_id)
            .take(Self::MAX_OPENED_BESIDE + 1)
            .collect();
        match files.split_first() {
            Some((first, rest)) => self.open_beside(first, rest).await,
            None => Ok(()),
        }
    }

    /// Get the LSP client that should handle `file_path`.
    ///
    /// The server set is *not* fixed at startup. [`Self::spawn_applicable`]
    /// only starts servers for the workspace roots the process was launched
    /// in (at startup when eager, else on demand); a request for a file
    /// anywhere lazily starts the right server, rooted at that file's own
    /// project. So the LSP tools work on any file, whatever directory the MCP
    /// server was started in.
    pub(super) async fn get_client_for_file(&self, file_path: &Path) -> Result<LspClient> {
        Ok(self.open_file(file_path).await?.0)
    }

    /// [`Self::get_client_for_file`], and whether `file_path` was just sent to
    /// the server — opened, or refreshed after changing on disk — so that the
    /// server is analysing it anew.
    pub(super) async fn open_file(&self, file_path: &Path) -> Result<(LspClient, bool)> {
        let extension = file_path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| {
                LspError::InvalidPath(format!(
                    "Cannot determine file extension: {}",
                    file_path.display()
                ))
            })?;

        // `.m` is MATLAB's as well as Objective-C's: clangd gets only the
        // Objective-C ones.
        if extension == "m" && !crate::languages::is_objective_c(file_path) {
            return Err(LspError::NoServerForFile(format!(
                "{} is not Objective-C (MATLAB uses .m too), and no language server here reads \
                 MATLAB",
                file_path.display()
            )));
        }
        let language_id = self
            .extension_map
            .get(extension)
            .ok_or_else(|| no_server_for(extension))?;
        self.restart_if_exited(language_id).await?;

        let (client, freshly_spawned) = match self.clients.get(language_id) {
            Some(client) => match self.cover_project(language_id, file_path).await? {
                Some(client) => (client, true),
                None => (client, false),
            },
            None => (
                self.start_server_for_file(language_id, file_path).await?,
                true,
            ),
        };

        // In use until the document is open and loaded; the request that
        // follows holds the client in use itself.
        let _in_use = client.in_use();

        // Opening or refreshing a document makes the server (re)analyse it, and
        // so does hearing of changes on disk. Let that settle, otherwise a
        // semantic request races the analysis and comes back empty —
        // indistinguishable, to an agent, from "no such symbol". The document
        // counts as refreshed whichever sent it anew: the refresh of the
        // documents changed on disk, or opening it.
        let key = uri_to_path(&path_to_uri(file_path)?);
        let sent_before = self.sent_version(&key).await;
        let synced = self.sync_with_disk(&client, language_id).await?;
        let opened = self.ensure_document_open(&client, file_path).await?;
        let refreshed = opened || self.sent_version(&key).await != sent_before;
        if synced || refreshed {
            client.wait_until_ready().await;
        }

        // `$/progress` alone is not a reliable "the file is loaded" signal: a
        // server reports progress in phases (rust-analyzer: metadata → cache
        // priming → indexing) with brief idle gaps between them, and
        // `wait_until_ready` returns on the first gap — before the file is mapped
        // to its crate. The first `documentSymbol` then comes back empty even
        // though the code is fine. Only on a cold spawn, probe until the server
        // actually knows the file (or a bounded budget elapses).
        if freshly_spawned {
            self.await_file_loaded(&client, file_path).await;
        }
        Ok((client, refreshed))
    }

    /// The version of the document at `key` last sent to its server, if it is
    /// open.
    async fn sent_version(&self, key: &str) -> Option<i32> {
        self.open_documents
            .read()
            .await
            .get(key)
            .map(|doc| doc.version)
    }

    /// Poll `documentSymbol` for a freshly spawned server until it returns
    /// symbols (the file is loaded) or [`Self::COLD_START_LOAD_BUDGET`] elapses.
    ///
    /// The common case — a non-empty first probe — returns immediately and adds
    /// no latency. A genuinely symbol-less file (empty/config) simply waits out
    /// the budget once, which is bounded well under the per-request timeout.
    pub(super) async fn await_file_loaded(&self, client: &LspClient, file_path: &Path) {
        let Ok(uri) = path_to_uri(file_path) else {
            return;
        };
        // The probe uses `documentSymbol` as a "file is loaded" proxy. A server
        // that doesn't implement it would never answer, burning the whole budget,
        // so skip the probe for those (`wait_until_ready` already ran).
        if !self
            .server_supports(&self.get_language_id(file_path), "documentSymbolProvider")
            .await
        {
            return;
        }
        let deadline = std::time::Instant::now() + Self::COLD_START_LOAD_BUDGET;
        loop {
            let params = lsp_types::DocumentSymbolParams {
                text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
                work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            };
            let resp: Result<Option<lsp_types::DocumentSymbolResponse>> =
                client.request("textDocument/documentSymbol", params).await;
            let loaded = matches!(
                &resp,
                Ok(Some(lsp_types::DocumentSymbolResponse::Flat(s))) if !s.is_empty()
            ) || matches!(
                &resp,
                Ok(Some(lsp_types::DocumentSymbolResponse::Nested(s))) if !s.is_empty()
            );
            if loaded || std::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(Self::COLD_START_LOAD_INTERVAL).await;
            client.wait_until_ready().await;
        }
    }

    /// Announce `file_path` to `client`, or refresh it if the file changed on disk.
    ///
    /// Servers answer position requests only for documents they know about, so
    /// this runs before every request rather than only before diagnostics. The
    /// file's modification time and size are tracked so an agent that edits a
    /// file and immediately asks for hover or diagnostics sees its own change,
    /// not the version cached when the document was first opened.
    /// Returns `true` if a `didOpen` or `didChange` was sent (i.e. the server
    /// now has fresh work to do), `false` if the document was already current.
    pub(super) async fn ensure_document_open(
        &self,
        client: &LspClient,
        file_path: &Path,
    ) -> Result<bool> {
        let tick = self
            .ticks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.send_document(client, file_path, Some(tick)).await
    }

    /// Send `file_path` to `client` as it is on disk, unless the server holds
    /// that already: opened, or changed when it holds an older text. A
    /// document whose modification time and size match what was sent, read
    /// within [`RACY_WINDOW`] of that time, is read again and compared by
    /// content. `tick` marks the document used; `None` leaves its use as it
    /// was, for a refresh no request asked for. Returns whether anything was
    /// sent.
    pub(super) async fn send_document(
        &self,
        client: &LspClient,
        file_path: &Path,
        tick: Option<u64>,
    ) -> Result<bool> {
        let uri = path_to_uri(file_path)?;
        let key = uri_to_path(&uri);
        let stamp = disk_stamp(file_path).await;

        // Fast path: already open and unchanged.
        if let Some(doc) = self.open_documents.write().await.get_mut(&key)
            && doc.current_by(stamp)
        {
            if let Some(tick) = tick {
                doc.last_used = tick;
            }
            return Ok(false);
        }

        let read_at = std::time::SystemTime::now();
        let text = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| LspError::Io(format!("Failed to read {}: {e}", file_path.display())))?;
        let hash = text_hash(&text);
        let language_id = document_language_id(file_path, &self.get_language_id(file_path));

        let mut open = self.open_documents.write().await;
        // Re-check: a concurrent caller may have sent this text while we read
        // the file, or the file may hold the text last sent.
        match open.get_mut(&key) {
            Some(doc) if doc.hash == hash => {
                doc.stamp = stamp;
                doc.read_at = read_at;
                if let Some(tick) = tick {
                    doc.last_used = tick;
                }
                return Ok(false);
            }
            Some(doc) => {
                let version = doc.version + 1;
                let change = lsp_types::DidChangeTextDocumentParams {
                    text_document: lsp_types::VersionedTextDocumentIdentifier {
                        uri: uri.clone(),
                        version,
                    },
                    content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text,
                    }],
                };
                client
                    .notify("textDocument/didChange", change)
                    .await
                    .map_err(|e| LspError::Io(format!("didChange failed: {e}")))?;
                *doc = OpenDocument {
                    version,
                    stamp,
                    hash,
                    read_at,
                    last_used: tick.unwrap_or(doc.last_used),
                    saved: None,
                };
            }
            None => {
                let params = lsp_types::DidOpenTextDocumentParams {
                    text_document: lsp_types::TextDocumentItem {
                        uri: uri.clone(),
                        language_id,
                        version: 1,
                        text,
                    },
                };
                client
                    .notify("textDocument/didOpen", params)
                    .await
                    .map_err(|e| LspError::Io(format!("didOpen failed: {e}")))?;
                let language_id = self.get_language_id(file_path);
                let last_used =
                    tick.unwrap_or_else(|| self.ticks.load(std::sync::atomic::Ordering::Relaxed));
                open.insert(
                    key.clone(),
                    OpenDocument {
                        version: 1,
                        stamp,
                        hash,
                        read_at,
                        last_used,
                        saved: None,
                    },
                );
                let stale = documents_to_close(
                    &open,
                    |path| self.get_language_id(Path::new(path)),
                    &language_id,
                    &key,
                    MAX_OPEN_DOCUMENTS,
                );
                for path in stale {
                    open.remove(&path);
                    if let Ok(uri) = path_to_uri(Path::new(&path)) {
                        let params = lsp_types::DidCloseTextDocumentParams {
                            text_document: lsp_types::TextDocumentIdentifier { uri },
                        };
                        if let Err(e) = client.notify("textDocument/didClose", params).await {
                            warn!("didClose for {path} failed: {e}");
                        }
                    }
                }
            }
        }

        Ok(true)
    }

    /// Tell `client`'s server that the open document of `file_path` is saved
    /// — it is: what Devscriptor sends is what is on disk — unless the server
    /// takes no saves, or was told already for the text it holds. A server may
    /// check a file only once it is saved: rust-analyzer runs `cargo check`
    /// (or clippy) then, and pushes what it finds. Returns the save of the
    /// text the server holds; `None` when it takes no saves, or the document
    /// is not open or changed on disk since it was sent.
    pub(super) async fn save_document(
        &self,
        client: &LspClient,
        language_id: &str,
        file_path: &Path,
    ) -> Result<Option<Saved>> {
        let Some(include_text) = self.takes_saves(language_id).await else {
            return Ok(None);
        };
        let uri = path_to_uri(file_path)?;
        let key = uri_to_path(&uri);
        let mut open = self.open_documents.write().await;
        let Some(doc) = open.get_mut(&key) else {
            return Ok(None);
        };
        if doc.saved.is_some() {
            return Ok(doc.saved);
        }
        let text = if include_text {
            match tokio::fs::read_to_string(file_path).await {
                Ok(text) if text_hash(&text) == doc.hash => Some(text),
                _ => return Ok(None),
            }
        } else {
            None
        };
        let saved = Saved {
            mark: client.diagnostics_mark(),
            checks: client.saving(),
        };
        let params = lsp_types::DidSaveTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            text,
        };
        client
            .notify("textDocument/didSave", params)
            .await
            .map_err(|e| LspError::Io(format!("didSave failed: {e}")))?;
        doc.saved = Some(saved);
        Ok(Some(saved))
    }

    /// Whether the running `language_id` server takes `didSave`, and if so,
    /// whether with the saved text: its `textDocumentSync.save`, `true` or
    /// `{ "includeText": … }`.
    async fn takes_saves(&self, language_id: &str) -> Option<bool> {
        let servers = self.servers.read().await;
        let save = servers
            .get(language_id)?
            .capabilities()
            .get("textDocumentSync")?
            .get("save")?
            .clone();
        match save {
            serde_json::Value::Bool(true) => Some(false),
            serde_json::Value::Object(options) => Some(
                options
                    .get("includeText")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            ),
            _ => None,
        }
    }

    /// Get language ID for a file path.
    pub(super) fn get_language_id(&self, path: &Path) -> String {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| self.extension_map.get(ext).cloned())
            .unwrap_or_else(|| "plaintext".to_string())
    }

    /// Whether the running server for `language_id` advertises an LSP capability
    /// (e.g. `documentSymbolProvider`). A capability that is absent, `null`, or
    /// `false` counts as unsupported; per the LSP spec a supported one is `true`
    /// or an options object. Returns `false` if no such server is running.
    pub(super) async fn server_supports(&self, language_id: &str, provider: &str) -> bool {
        let servers = self.servers.read().await;
        servers
            .get(language_id)
            .is_some_and(|s| capability_supported(s.capabilities(), provider))
    }

    /// `client.request(method, params)` for a request about `file`. Where the
    /// server declares no capability for `method` ([`declaring_capability`])
    /// and the request fails, it fails as [`LspError::Unsupported`]: a server
    /// answers what it does not offer with an error of its own, an opaque
    /// `-32601 Unhandled method` or worse (jdtls: `-32603 Internal error`). A
    /// server that answers all the same is taken at its word: clangd answers
    /// type hierarchies it does not declare.
    pub(super) async fn request_feature<P, R>(
        &self,
        client: &LspClient,
        file: &Path,
        method: &str,
        params: P,
    ) -> Result<R>
    where
        P: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        match client.request(method, params).await {
            Ok(response) => Ok(response),
            Err(error) => Err(self.unsupported_unless_declared(file, method, error).await),
        }
    }

    /// `error`, a failure of `method` on the server of `file`, or
    /// [`LspError::Unsupported`] where the server declares no capability for
    /// `method`, or answers that it has no such method (`-32601`), whatever
    /// it declares: sql-language-server declares renames it has no handler
    /// for. A broken connection is not a missing feature, and stays.
    pub(super) async fn unsupported_unless_declared(
        &self,
        file: &Path,
        method: &str,
        error: LspError,
    ) -> LspError {
        let Some((provider, feature)) = declaring_capability(method) else {
            return error;
        };
        if matches!(
            error,
            LspError::ServerClosed | LspError::Transport(_) | LspError::Shutdown
        ) {
            return error;
        }
        let language_id = self.get_language_id(file);
        if matches!(
            error,
            LspError::LspServerError {
                code: LSP_METHOD_NOT_FOUND,
                ..
            }
        ) {
            return LspError::Unsupported {
                language_id,
                feature: feature.to_string(),
            };
        }
        let servers = self.servers.read().await;
        match servers.get(&language_id) {
            Some(server) if !capability_supported(server.capabilities(), provider) => {
                LspError::Unsupported {
                    language_id,
                    feature: feature.to_string(),
                }
            }
            _ => error,
        }
    }

    /// Turn a request the server cannot handle into a clear up-front error
    /// instead of an opaque `-32601 Unhandled method` from the wire. Best-effort:
    /// if the server is not (yet) known, the request is allowed to proceed.
    pub(super) async fn ensure_capability(
        &self,
        path: &Path,
        provider: &str,
        feature: &str,
    ) -> Result<()> {
        let language_id = self.get_language_id(path);
        let servers = self.servers.read().await;
        if let Some(server) = servers.get(&language_id)
            && !capability_supported(server.capabilities(), provider)
        {
            return Err(LspError::Unsupported {
                language_id,
                feature: feature.to_string(),
            });
        }
        Ok(())
    }
}
