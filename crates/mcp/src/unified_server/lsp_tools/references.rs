//! References and calls: the files that use a symbol opened in its server
//! first, and the index filling in what the server did not report.

use super::*;

impl UnifiedMcpServer {
    /// `get_references`, after opening in its server the files the index says
    /// use the symbol ([`Self::open_files_using`]).
    pub(super) async fn get_references_impl(
        &self,
        translator: &Arc<Translator>,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let mut args = args;
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        self.resolve_symbol_args(&mut args).await?;
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let line = args
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let character = args
            .get("character")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let include_declaration = args
            .get("include_declaration")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let name =
            name.or_else(|| identifier_at(std::path::Path::new(&file_path), line, character));
        if let Some(name) = &name {
            self.open_files_using(translator, name, &file_path).await;
        }
        let mut result = translator
            .handle_references(ReferencesParams {
                file_path: file_path.clone(),
                line,
                character,
                include_declaration,
            })
            .await;
        if let (Ok(references), Some(name)) = (&mut result, &name) {
            self.add_index_uses(translator, name, &file_path, references)
                .await;
        }
        json_result(result)
    }

    /// The folders serving `file`, and the reference family of its language:
    /// where the index's uses of a symbol of `file` are looked for, and in
    /// which languages they can be (`reference_family`).
    pub(super) async fn index_scope(
        translator: &Arc<Translator>,
        file: &str,
    ) -> Option<(String, Vec<std::path::PathBuf>)> {
        use devscriptor_core::parser::language_config::{
            detect_language_from_filename, reference_family,
        };
        let language = detect_language_from_filename(file)?;
        let folders = translator
            .folders_serving(std::path::Path::new(file))
            .await
            .ok()?;
        Some((reference_family(&language).to_string(), folders))
    }

    /// Add to `references` the uses of `name` the index records under the
    /// folders serving `file`, in languages that can name `file`'s symbols
    /// (`reference_family`), that the server did not report. A server answers
    /// for what it reads: typescript-language-server leaves out the calls in
    /// `.vue` and `.svelte` files, and Metals without a build those in other
    /// files, so an answer can look complete when it is not.
    pub(super) async fn add_index_uses(
        &self,
        translator: &Arc<Translator>,
        name: &str,
        file: &str,
        references: &mut devscriptor_lsp::mcp::tools::ReferencesResult,
    ) {
        use devscriptor_core::parser::language_config::reference_family;
        let Some((family, folders)) = Self::index_scope(translator, file).await else {
            return;
        };
        let state = self.core.state();
        let names = [name.to_string()];
        // Read from an index current for the files of the sites: a site the
        // index took from an older version of its file may be gone.
        let read = crate::freshness::fresh_answer(state, || async {
            let mut sites = Vec::new();
            for folder in &folders {
                match state.database().get_reference_sites(folder, &names).await {
                    Ok(found) => sites.extend(
                        found
                            .into_iter()
                            .filter(|(_, _, language)| reference_family(language) == family)
                            .map(|(site, line, _)| (site, line)),
                    ),
                    Err(e) => {
                        tracing::debug!("No index uses of {name} under {}: {e}", folder.display())
                    }
                }
            }
            let files = sites.iter().map(|(site, _)| site.clone()).collect();
            Ok::<_, std::convert::Infallible>((sites, files))
        })
        .await;
        let Ok((sites, _)) = read;
        references.add_index_uses(name, sites);
    }

    /// Add to `calls` the calls of `name` the index records under the folders
    /// serving `file`, in languages that can name `file`'s symbols, that the
    /// server did not report, each with what it is made in: asked for the
    /// callers of a function a Vue component calls, typescript-language-server
    /// gives only those in the files it reads.
    pub(super) async fn add_index_calls(
        &self,
        translator: &Arc<Translator>,
        name: &str,
        file: &str,
        calls: &mut devscriptor_lsp::mcp::tools::IncomingCallsResult,
    ) {
        use devscriptor_core::parser::language_config::reference_family;
        let Some((family, folders)) = Self::index_scope(translator, file).await else {
            return;
        };
        let state = self.core.state();
        let names = [name.to_string()];
        // Read from an index current for the files of the calls: a call the
        // index took from an older version of its file may be gone.
        let read = crate::freshness::fresh_answer(state, || async {
            let mut sites = Vec::new();
            for folder in &folders {
                match state.database().get_call_sites(folder, &names).await {
                    Ok(found) => sites.extend(
                        found
                            .into_iter()
                            .filter(|(_, _, language, _)| reference_family(language) == family)
                            .map(|(site, line, _, caller)| (site, line, caller)),
                    ),
                    Err(e) => {
                        tracing::debug!("No index calls of {name} under {}: {e}", folder.display())
                    }
                }
            }
            let files = sites.iter().map(|(site, ..)| site.clone()).collect();
            Ok::<_, std::convert::Infallible>((sites, files))
        })
        .await;
        let Ok((sites, _)) = read;
        calls.add_index_calls(name, sites);
    }

    /// Open, in the server answering for `file`, the files under its folders
    /// that the index says use `name` — call it, pass it, use it as a type or
    /// import it — before it is asked for the references or callers of
    /// `name`: a server that looks only in the projects of open files finds
    /// theirs then ([`Translator::open_beside`]): typescript-language-server, for
    /// one, gives a function's references without the calls a test file makes
    /// of it until the test file is open.
    pub(super) async fn open_files_using(
        &self,
        translator: &Arc<Translator>,
        name: &str,
        file: &str,
    ) {
        let file = std::path::Path::new(file);
        let Ok(folders) = translator.folders_serving(file).await else {
            return;
        };
        let database = self.core.state().database();
        let names = [name.to_string()];
        let mut files = std::collections::BTreeSet::new();
        for folder in &folders {
            match database.get_sources_referring_to(folder, &names).await {
                Ok(sources) => files.extend(
                    sources
                        .into_iter()
                        .map(|(_, path)| std::path::PathBuf::from(path)),
                ),
                Err(e) => tracing::debug!("No files using {name} under {}: {e}", folder.display()),
            }
        }
        let files: Vec<std::path::PathBuf> = files.into_iter().collect();
        if let Err(e) = translator.open_beside(file, &files).await {
            tracing::debug!("Not opening the files using {name}: {e}");
        }
    }
}
