use std::{
    collections::{BTreeMap, HashMap},
    env, fmt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use std::sync::Arc;

use zg_host_native::{NativeScanner, ScanRequest, TaskControl, WorkspaceScannerPort};

use crate::{
    EngineError,
    api::{
        index::{IndexOptions, IndexResult, options::EmbeddingModelSpec},
        info::{
            InfoOptions, InfoResult,
            result::{IndexCompatibility, InfoSource, WorkspaceIndexInfo, WorkspaceIndexPolicy},
        },
    },
    domain::{
        IndexDescriptor, IndexState, Workspace,
        model::{Device, ModelConfig},
    },
    models::{
        ModelError, ModelRuntimeLease, ModelRuntimeManager, ModelRuntimeRequest,
        ResolveEmbeddingReferenceOptions, resolve_embedding_reference,
    },
    storage::{IndexStore, types::WorkspaceIndexStorageOptions},
    workspace::{
        CURRENT_INDEX_VERSION,
        build::{
            WorkspaceBuild, discard_build, has_build, has_generation_storage, prepare_build,
            publish_build, read_build_registration, recover_build, recover_build_for_rebuild,
        },
        layout::{
            WorkspaceIndexLocation, find_nearest_workspace, reset_workspace_index,
            workspace_index_location,
        },
        lock::{FileLock, LockMode, LockWait},
        manifest::{
            ManifestState, WorkspaceManifest, inspect_workspace_manifest, read_workspace_manifest,
            write_workspace_manifest,
        },
        registry::WorkspaceRegistry,
    },
};

use super::pipeline::{
    IndexEmbeddingRuntime, IndexingContext, get_workspace_index_status, index_workspace,
};

const DEFAULT_LOCAL_EMBEDDING: &str = "local/gte-modernbert-base";

#[derive(Clone)]
pub(crate) struct WorkspaceIndexService {
    pub(crate) writers: crate::pipelines::indexed_search::writer::WriterContexts,
    scanner: NativeScanner,
    registry: Option<WorkspaceRegistry>,
    #[cfg(test)]
    _registry_directory: Option<Arc<tempfile::TempDir>>,
    #[cfg(test)]
    fail_index_completion: bool,
}

impl WorkspaceIndexService {
    pub(crate) fn new() -> Self {
        Self {
            writers: crate::pipelines::indexed_search::writer::WriterContexts::default(),
            scanner: NativeScanner::default(),
            registry: None,
            #[cfg(test)]
            _registry_directory: None,
            #[cfg(test)]
            fail_index_completion: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_registry() -> Self {
        let directory = Arc::new(tempfile::tempdir().expect("test workspace registry"));
        Self {
            writers: crate::pipelines::indexed_search::writer::WriterContexts::default(),
            scanner: NativeScanner::default(),
            registry: Some(
                WorkspaceRegistry::at(directory.path().join("workspaces.json"))
                    .expect("registry path"),
            ),
            _registry_directory: Some(directory),
            fail_index_completion: false,
        }
    }

    fn registry(&self) -> Result<WorkspaceRegistry, EngineError> {
        self.registry
            .clone()
            .map_or_else(WorkspaceRegistry::global, Ok)
    }

    /// A registry rename is authoritative; replay it into metadata after a crash.
    pub(in crate::pipelines) fn reconcile_name(
        &self,
        manifest: &mut WorkspaceManifest,
    ) -> Result<(), EngineError> {
        let registry = self.registry()?;
        if let Some(name) = registry.name_for_root(&manifest.workspace.root)? {
            manifest.workspace.name = name;
        } else if let Some((name, _)) = moved_registration(&registry, manifest)? {
            manifest.workspace.name = name;
        } else if let Some(previous) = registry.root_for_name(&manifest.workspace.name)? {
            return Err(EngineError::invalid_argument(format!(
                "workspace name '{}' is already registered at {}; use zg --index --name to choose another name",
                manifest.workspace.name,
                previous.display()
            )));
        }
        Ok(())
    }

    /// Resolves the scan rules, the workspace glob filter and, for a new
    /// workspace, the model its dominant source language prefers.
    async fn index_profile(
        &self,
        existing: Option<&WorkspaceManifest>,
        options: &IndexOptions,
        root: &Path,
    ) -> Result<Option<&'static str>, EngineError> {
        let scan = resolve_scan(existing, options);
        let selection = crate::file_selection::GlobMatcher::new(root, &scan.globs)?;
        dominant_language_model(&self.scanner, existing, options, root, &scan, &selection).await
    }

    fn register_name(
        &self,
        root: &Path,
        existing: Option<&WorkspaceManifest>,
        abandoned: Option<&(String, PathBuf)>,
        requested: Option<&str>,
    ) -> Result<String, EngineError> {
        let registry = self.registry()?;
        let requested = requested.map(str::to_owned);
        if let Some(current) = registry.name_for_root(root)? {
            let name = requested.unwrap_or_else(|| current.clone());
            registry.rename(&current, &name, root)?;
            return Ok(name);
        }
        let registration = existing
            .map(|manifest| (&manifest.workspace.name, &manifest.recorded_root))
            .or_else(|| abandoned.map(|(name, root)| (name, root)));
        if let Some((name, previous_root)) = registration
            && let Some((previous_name, previous_root)) =
                moved_registration_for(&registry, name, previous_root, root)?
        {
            registry.relocate(&previous_name, &previous_root, root)?;
            let name = requested.unwrap_or_else(|| previous_name.clone());
            registry.rename(&previous_name, &name, root)?;
            return Ok(name);
        }
        let name = requested
            .or_else(|| existing.map(|manifest| manifest.workspace.name.clone()))
            .or_else(|| abandoned.map(|(name, _)| name.clone()))
            .unwrap_or_else(|| workspace_name(root));
        registry.register(&name, root)?;
        Ok(name)
    }

    pub(crate) async fn index(
        &self,
        models: &ModelRuntimeManager,
        mut options: IndexOptions,
    ) -> Result<IndexResult, EngineError> {
        if let Some(name) = options.name.as_deref() {
            Workspace::validate_name(name)?;
        }
        normalize_model_paths(&mut options)?;
        let requested_root = resolve_root(options.root.as_deref())?;
        validate_workspace_root(&requested_root)?;
        let location = workspace_index_location(&requested_root)?;
        options.root = Some(location.root.clone());
        let home_lock = Arc::new(
            LockWait::new(options.signal.as_ref(), options.lock_timeout_ms)?
                .acquire(
                    &location.home,
                    LockMode::Write,
                    if options.rebuild {
                        "index.rebuild"
                    } else {
                        "index"
                    },
                )
                .await?,
        );
        // Check the on-disk version before any registry, recovery or storage mutation.
        let mut existing =
            inspect_workspace_manifest(&location.home)?.into_manifest(options.rebuild)?;
        let abandoned = match read_build_registration(&location.home) {
            Ok(registration) => registration,
            Err(_) if options.rebuild => None,
            Err(error) => return Err(error),
        };
        // Retain only workspace naming/relocation information from a crashed
        // first build. Its model settings and computed data are never reused.
        let name = self.register_name(
            &location.root,
            existing.as_ref(),
            abandoned.as_ref(),
            options.name.as_deref(),
        )?;
        if options.rebuild {
            recover_build_for_rebuild(&location.home)?;
        } else {
            recover_build(&location.home)?;
        }
        options.name = Some(name.clone());
        if let Some(active) = &mut existing {
            active.workspace.name = name;
        }
        let rebuilding = options.rebuild
            || existing
                .as_ref()
                .is_none_or(|manifest| !is_indexed(manifest));
        // Validate before model acquisition; invalid globs must not trigger downloads or inference.
        let language_default = self
            .index_profile(existing.as_ref(), &options, &location.root)
            .await?;
        if options.reset_paths
            || options.scan != crate::api::index::options::ScanRulesUpdate::default()
        {
            // A changed selection can admit files outside a watcher's narrow change scope.
            options.changes.clear();
        }
        let IndexModels {
            runtimes: acquired,
            descriptor,
        } = acquire_index_models(models, existing.as_ref(), &options, language_default)?;
        // Acquisition precedes the identity refusal below: construction is
        // lazy and runtime-manager-cached, while the true backend identity is
        // only known once the backend exists. Globs validate before it.
        let mut descriptor = descriptor;
        check_incremental_identity(
            existing.as_ref(),
            rebuilding,
            &mut descriptor,
            &acquired,
            options.signal.clone(),
            options.on_progress.clone(),
        )
        .await?;
        let mut manifest = index_manifest(
            &location,
            existing.as_ref(),
            &options,
            &acquired,
            descriptor,
        )?;
        let build = if rebuilding {
            // Fresh builds always cover the full configured workspace.
            options.changes.clear();
            let build = prepare_build(manifest)?;
            manifest = build.target.clone();
            Some(build)
        } else {
            None
        };
        let pending = build.clone();
        let result = self
            .run_index(manifest, build, acquired, options, Arc::clone(&home_lock))
            .await;
        if result.is_err()
            && let Some(build) = pending
        {
            // Storage handles have been released. Preserve the original error if
            // cleanup also fails; its build record lets the next writer retry.
            let _ = discard_build(&build);
        }
        result
    }

    async fn run_index(
        &self,
        mut manifest: WorkspaceManifest,
        build: Option<WorkspaceBuild>,
        models: Vec<ModelRuntimeLease>,
        options: IndexOptions,
        home_lock: Arc<FileLock>,
    ) -> Result<IndexResult, EngineError> {
        // The storage schema records the same identity the manifest target
        // carries, so a generation published after a provider fallback keeps
        // one consistent identity across both records.
        let opened_embeddings: Vec<_> = manifest.embeddings().to_vec();
        let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadWrite {
            storage_path: manifest.storage_home(),
            embeddings: opened_embeddings.clone(),
        })?;
        let writer = self.writers.register(
            crate::pipelines::indexed_search::writer::WriterSession::new(
                storage,
                models,
                manifest.clone(),
                home_lock,
            ),
            build.is_none(),
        );
        let embedding_models = writer
            .session
            .models
            .iter()
            .map(|model| model as &dyn IndexEmbeddingRuntime)
            .collect::<Vec<_>>();
        let result = index_workspace(&IndexingContext {
            workspace_index: &manifest.workspace,
            storage: &writer.session.storage,
            scanner: &self.scanner,
            embedding_models: &embedding_models,
            embedding_concurrency: options.embedding_concurrency,
            on_progress: options.on_progress,
            signal: options.signal.clone(),
            changes: &options.changes,
            // An incremental update appends to a live generation and must not
            // mix runtimes, so the pipeline enforces the recorded identity
            // per batch before committing. A fresh build accepts its starting
            // identity and relabels its staging generation at publication.
            expected_backend_identity: if build.is_some() {
                None
            } else {
                opened_embeddings
                    .first()
                    .and_then(crate::domain::model::EmbeddingModelInfo::backend_identity)
            },
        })
        .await;
        #[cfg(test)]
        let result = result.and_then(|indexed| {
            // Exercise service cleanup when the indexing pipeline reports a failed checkpoint.
            if self.fail_index_completion {
                Err(EngineError::storage_failure("injected checkpoint failure"))
            } else {
                Ok(indexed)
            }
        });
        // Closing precedes publication: all checkpoints and native handles belong
        // to the completed generation before the active manifest can select it.
        // Capture the runtime's live identity first: the models that produced
        // this generation's vectors are owned by the writer session.
        let runtime_identity = writer.session.models.first().and_then(|model| {
            model
                .backend_identity()
                .or_else(|| model.info().backend_identity())
        });
        writer.retire().await;
        let close_result = writer.session.storage.close();
        let indexed = result?;
        close_result?;
        if options
            .signal
            .as_ref()
            .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
        {
            return Err(EngineError::cancelled(
                "indexing was cancelled before publication",
            ));
        }
        let now = epoch_millis();
        let opened_identity = opened_embeddings
            .first()
            .and_then(crate::domain::model::EmbeddingModelInfo::backend_identity);
        if let Some(mut build) = build {
            // A fresh build may relabel itself: the storage schema was
            // written before the models loaded, so an accelerated provider
            // that fell back to CPU while embedding is corrected here, before
            // the generation is published. Storage handles are closed and
            // the home write lock is still held.
            record_runtime_backend_identity(&mut manifest, runtime_identity.clone());
            if opened_identity != runtime_identity {
                IndexStore::rewrite_schema(&manifest.storage_home(), manifest.embeddings())?;
            }
            build.target = manifest.clone();
            publish_build(build, now)?;
        } else {
            // An incremental update appends to the live generation in place.
            // The pipeline already refused any batch whose runtime identity
            // deviated before committing it; this end-of-run check is a
            // backstop for drift that bypassed the batches (for example a
            // sticky fallen-back snapshot on a run that embedded nothing).
            if opened_identity != runtime_identity {
                return Err(crate::domain::model::mid_run_backend_mismatch(
                    opened_identity.as_ref(),
                    runtime_identity.as_ref(),
                ));
            }
            manifest.record_update(now);
            write_workspace_manifest(&manifest.path, &manifest)?;
        }
        Ok(indexed)
    }

    pub(in crate::pipelines) async fn workspace_needs_refresh(
        &self,
        location: &WorkspaceIndexLocation,
        wait: &LockWait<'_>,
    ) -> Result<bool, EngineError> {
        let _lock = wait
            .acquire(&location.home, LockMode::Read, "context.refresh")
            .await?;
        let Some(manifest) = read_workspace_manifest(&location.home)? else {
            return Ok(false);
        };
        if !is_indexed(&manifest) || manifest.workspace.index == IndexState::Disabled {
            return Ok(false);
        }
        let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
            storage_path: manifest.storage_home(),
        })?;
        let status =
            get_workspace_index_status(&manifest.workspace, &storage, &self.scanner, None).await;
        let close = storage.close();
        let status = status?;
        close?;
        Ok(status.files_added > 0
            || status.files_modified > 0
            || status.files_deleted > 0
            || status.files_pending > 0
            || status.files_failed > 0)
    }

    pub(crate) async fn info(&self, options: InfoOptions) -> Result<InfoResult, EngineError> {
        let requested_root = resolve_root(options.root.as_deref())?;
        let requested_location = workspace_index_location(&requested_root)?;
        let Some(location) = find_nearest_workspace(&requested_root)? else {
            return Ok(unindexed_info(
                requested_location,
                WorkspaceIndexPolicy::Uninitialized,
            ));
        };
        let _lock = LockWait::new(None, None)?
            .acquire(&location.home, LockMode::Read, "info")
            .await?;
        let mut manifest = match inspect_workspace_manifest(&location.home)? {
            ManifestState::Current(manifest) => manifest,
            ManifestState::Missing => {
                return Ok(unindexed_info(
                    location,
                    WorkspaceIndexPolicy::Uninitialized,
                ));
            }
            ManifestState::RebuildRequired {
                actual_version,
                reason,
            } => {
                let mut info = unindexed_info(location, WorkspaceIndexPolicy::Enabled);
                info.compatibility = IndexCompatibility::RebuildRequired {
                    actual_version,
                    expected_version: CURRENT_INDEX_VERSION,
                    reason,
                };
                info.suggestion = Some("rebuild the index with `zg --index --rebuild`".to_owned());
                return Ok(info);
            }
        };
        self.reconcile_name(&mut manifest)?;
        let metadata_indexed = is_indexed(&manifest);
        let storage_exists = IndexStore::exists(&manifest.storage_home())?;
        let indexed = metadata_indexed && storage_exists;
        let status = if options.include_status && indexed {
            let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
                storage_path: manifest.storage_home(),
            })?;
            let status =
                get_workspace_index_status(&manifest.workspace, &storage, &self.scanner, None)
                    .await;
            let close = storage.close();
            let status = status?;
            close?;
            Some(status)
        } else {
            None
        };

        Ok(InfoResult {
            compatibility: if metadata_indexed {
                IndexCompatibility::Compatible {
                    version: CURRENT_INDEX_VERSION,
                }
            } else {
                IndexCompatibility::Unbuilt
            },
            root: location.root,
            indexed,
            index_policy: (&manifest.workspace.index).into(),
            home: location.home,
            index_path: manifest.storage_home().join("storage"),
            source: if indexed {
                InfoSource::Index
            } else {
                InfoSource::Unindexed
            },
            workspace_index: Some(workspace_info(&manifest)),
            status,
            suggestion: workspace_suggestion(&manifest, indexed),
        })
    }

    pub(crate) async fn drop_index(&self, options: &InfoOptions) -> Result<bool, EngineError> {
        let location = workspace_index_location_from_option(options.root.as_deref())?;
        let registry = self.registry()?;
        let name = registry.name_for_root(&location.root)?;
        if name.is_none() && !workspace_has_index_data(&location)? {
            return Ok(false);
        }
        if !location.root.try_exists().map_err(|error| {
            EngineError::from_io(
                format!("inspect workspace root {}", location.root.display()),
                &error,
            )
        })? {
            return registry.unregister_missing(&location.root);
        }
        let _lock = LockWait::new(None, None)?
            .acquire(&location.home, LockMode::Write, "index.drop")
            .await?;
        let registration = if let Some(name) = registry.name_for_root(&location.root)? {
            Some((name, location.root.clone()))
        } else {
            // Corrupt metadata must not prevent explicit cleanup. Valid metadata
            // also lets a moved workspace release its previous registry location.
            if let Some(manifest) = read_workspace_manifest(&location.home).ok().flatten() {
                moved_registration(&registry, &manifest)?
            } else if let Some((name, root)) =
                read_build_registration(&location.home).ok().flatten()
            {
                moved_registration_for(&registry, &name, &root, &location.root)?
            } else {
                None
            }
        };
        let has_data = workspace_has_index_data(&location)?;
        if has_data {
            reset_workspace_index(&location)?;
        }
        if let Some((name, registered_root)) = &registration {
            registry.unregister(name, registered_root)?;
        }
        Ok(has_data || registration.is_some())
    }
}

fn workspace_has_index_data(location: &WorkspaceIndexLocation) -> Result<bool, EngineError> {
    Ok(location.manifest_path.exists()
        || has_build(&location.home)
        || location.home.join("files.zvec").exists()
        || location.home.join("index.zvec").exists()
        || has_generation_storage(&location.home)?)
}

/// Check an incremental update against the previous generation. The
/// descriptor's identity is construction-time; when it disagrees with the
/// previous generation, retry against the loaded runtime's live identity
/// before refusing, so a machine whose accelerated provider persistently
/// falls back to CPU can still refresh the generation it published under
/// that live identity.
async fn check_incremental_identity(
    existing: Option<&WorkspaceManifest>,
    rebuilding: bool,
    descriptor: &mut crate::domain::IndexDescriptor,
    acquired: &[ModelRuntimeLease],
    signal: Option<tokio_util::sync::CancellationToken>,
    on_progress: Option<crate::api::index::progress::IndexProgressReporter>,
) -> Result<(), EngineError> {
    if rebuilding {
        return Ok(());
    }
    let Some(previous) = existing.and_then(|manifest| manifest.workspace.index.descriptor()) else {
        return Ok(());
    };
    let model = acquired
        .first()
        .expect("indexing acquires exactly one model");
    if let Err(error) = previous.ensure_index_compatible(descriptor)
        && error.code() == EngineError::INDEX_BACKEND_MISMATCH
    {
        model
            .prepare(
                crate::models::EmbeddingPrepareOptions {
                    signal,
                    on_progress: None,
                },
                on_progress.map(super::model_progress::for_index),
            )
            .await
            .map_err(ModelError::into_engine_error)?;
        if let Some(identity) = model
            .backend_identity()
            .or_else(|| model.info().backend_identity())
            && let Some(info) = descriptor.embeddings.first_mut()
        {
            info.record_backend_identity(identity);
        }
        return previous.ensure_index_compatible(descriptor);
    }
    Ok(())
}

/// Refusal for an incremental update whose runtime changed identity after
/// the storage opened. The live generation already holds vectors from both
/// runtimes; it keeps its recorded identity, and the structured error names
/// both so the caller can rebuild.
/// Record the identity of the runtime that actually produced this build's
/// vectors before the generation is published. A session that fell back to
/// CPU after the manifest target was created must not publish its
/// accelerated-provider label.
fn record_runtime_backend_identity(
    manifest: &mut WorkspaceManifest,
    identity: Option<crate::domain::model::BackendIdentity>,
) {
    let Some(identity) = identity else {
        return;
    };
    let Some(info) = manifest
        .workspace
        .index
        .descriptor()
        .and_then(|descriptor| descriptor.embeddings.first())
    else {
        return;
    };
    if info.backend.as_deref() == Some(identity.backend.as_str())
        && info.dtype.as_deref() == Some(identity.dtype.as_str())
    {
        return;
    }
    let mut updated = info.clone();
    updated.record_backend_identity(identity);
    manifest.workspace.index = IndexState::Enabled(IndexDescriptor::single(updated));
}

fn index_manifest(
    location: &WorkspaceIndexLocation,
    active: Option<&WorkspaceManifest>,
    options: &IndexOptions,
    models: &[ModelRuntimeLease],
    descriptor: IndexDescriptor,
) -> Result<WorkspaceManifest, EngineError> {
    let now = epoch_millis();
    let workspace = Workspace {
        name: options
            .name
            .clone()
            .or_else(|| active.map(|value| value.workspace.name.clone()))
            .unwrap_or_else(|| workspace_name(&location.root)),
        root: location.root.clone(),
        scan: resolve_scan(active, options),
        index: IndexState::Enabled(descriptor),
        created_epoch_ms: active.map_or(now, |value| value.workspace.created_epoch_ms),
        updated_epoch_ms: now,
    };
    let runtime = models
        .iter()
        .map(|model| {
            Ok((
                model.info().model.reference(),
                embedding_runtime(active, options, model)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, EngineError>>()?;
    let mut manifest = WorkspaceManifest::new(
        workspace,
        location.home.clone(),
        Some(CURRENT_INDEX_VERSION),
        runtime,
    )?;
    manifest.storage_generation = active.and_then(|value| value.storage_generation.clone());
    Ok(manifest)
}

impl Default for WorkspaceIndexService {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for WorkspaceIndexService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceIndexService")
            .field("scanner", &self.scanner)
            .finish_non_exhaustive()
    }
}

/// Chooses the model for a workspace that has no model of its own yet.
///
/// A workspace embeds text with one model, so a polyglot repository has to be
/// served by a single choice. The dominant source language picks its measured
/// best model and the other languages stay indexed under it. Explicit
/// requests, a stored model, and a configured global default all win over
/// this inference; only a fresh workspace with no other signal is profiled.
async fn dominant_language_model(
    scanner: &dyn WorkspaceScannerPort,
    existing: Option<&WorkspaceManifest>,
    options: &IndexOptions,
    root: &Path,
    scan: &crate::domain::ScanRules,
    selection: &crate::file_selection::GlobMatcher,
) -> Result<Option<&'static str>, EngineError> {
    if options.embedding.is_some() || existing.and_then(|manifest| manifest.embedding()).is_some() {
        return Ok(None);
    }
    let config = crate::config::read()?;
    let configured = crate::config::string(&config, &["defaults", "embedding"]);
    if configured_default_wins(configured.as_deref(), options.implicit) {
        return Ok(None);
    }

    language_model_from_scan(scanner, options, root, scan, selection).await
}

/// Scans the workspace and maps its file profile to a model. Split from the
/// configuration gate so the scanner-to-profile path is testable.
async fn language_model_from_scan(
    scanner: &dyn WorkspaceScannerPort,
    options: &IndexOptions,
    root: &Path,
    scan: &crate::domain::ScanRules,
    selection: &crate::file_selection::GlobMatcher,
) -> Result<Option<&'static str>, EngineError> {
    let root_spec = crate::file_selection::ScanPolicy::root_spec(root, scan)?;
    let snapshot = scanner
        .discover(
            &ScanRequest {
                roots: vec![root_spec],
                scope_paths: Vec::new(),
            },
            &TaskControl::new(options.signal.clone().unwrap_or_default()),
        )
        .await
        .map_err(super::pipeline::map_host_error)?;

    Ok(profile_model(
        snapshot
            .files
            .into_iter()
            .map(|file| (file.relative_path, file.size_bytes)),
        selection,
    ))
}

/// Reports whether a configured default will decide this build, making the
/// language profile irrelevant. An implicit build ignores a remote default
/// (implicit builds never select remote models), so profiling still applies.
fn configured_default_wins(configured: Option<&str>, implicit: bool) -> bool {
    configured.is_some_and(|reference| !implicit || reference.starts_with("local/"))
}

/// Selects a model from a file listing. Split out from the scanner call so the
/// weighting and glob filtering are testable without a workspace.
fn profile_model(
    files: impl IntoIterator<Item = (PathBuf, u64)>,
    selection: &crate::file_selection::GlobMatcher,
) -> Option<&'static str> {
    let mut weights = HashMap::new();
    let mut unmapped_bytes = 0_u64;
    for (path, size) in files {
        if !selection.matches_path(&path) {
            continue;
        }
        match super::language_profile::language_for_path(&path) {
            Some(language) => *weights.entry(language).or_default() += size,
            // A language with no measured preference still competes for
            // dominance: if it outweighs every mapped language, no mapping may
            // speak for the repository and the generalist applies.
            None if super::language_profile::is_source_like(&path) => unmapped_bytes += size,
            None => {}
        }
    }
    let dominant =
        super::language_profile::dominant_language(weights.iter().map(|(k, v)| (*k, *v)));
    match dominant {
        Some(language) if weights.get(&language).copied().unwrap_or_default() > unmapped_bytes => {
            Some(language.preferred_model())
        }
        _ => None,
    }
}

fn normalize_model_paths(options: &mut IndexOptions) -> Result<(), EngineError> {
    if let Some(cache_dir) = options
        .embedding
        .as_mut()
        .and_then(|spec| spec.cache_dir.as_mut())
    {
        *cache_dir = std::path::absolute(&*cache_dir).map_err(|error| {
            EngineError::from_io("failed to resolve model cache directory", &error)
        })?;
    }
    Ok(())
}

struct IndexModels {
    runtimes: Vec<ModelRuntimeLease>,
    descriptor: IndexDescriptor,
}

fn acquire_index_models(
    models: &ModelRuntimeManager,
    existing: Option<&WorkspaceManifest>,
    options: &IndexOptions,
    language_default: Option<&'static str>,
) -> Result<IndexModels, EngineError> {
    let model = acquire_model(models, existing, options, language_default)?;
    let descriptor = IndexDescriptor::single(model.info().clone());
    descriptor.validate()?;
    Ok(IndexModels {
        runtimes: vec![model],
        descriptor,
    })
}

/// Rejects a pinned model revision: the catalog-backed runtime only supports
/// the revision it ships.
fn reject_revision_override(options: &IndexOptions) -> Result<(), EngineError> {
    if options
        .embedding
        .as_ref()
        .and_then(|embedding| embedding.revision.as_ref())
        .is_some()
    {
        return Err(EngineError::unsupported(
            "embedding revision overrides are not supported by the catalog-backed runtime",
        ));
    }
    Ok(())
}

fn acquire_model(
    models: &ModelRuntimeManager,
    existing: Option<&WorkspaceManifest>,
    options: &IndexOptions,
    language_default: Option<&'static str>,
) -> Result<ModelRuntimeLease, EngineError> {
    reject_revision_override(options)?;
    let reference = embedding_reference(
        existing,
        options.embedding.as_ref(),
        language_default,
        options.implicit,
        options.suppress_environment,
    )?;
    let config = crate::config::read()?;
    let local = reference.starts_with("local/");
    if (!local && options.device.is_some()) || (local && options.endpoint.is_some()) {
        return Err(EngineError::invalid_argument(
            "device requires a local model; endpoint requires a remote model",
        ));
    }
    let existing_runtime =
        existing.and_then(|manifest| manifest.embedding_runtimes.get(&reference));
    let api_key = if local {
        None
    } else {
        options
            .api_key
            .clone()
            .or_else(|| existing_runtime.and_then(|runtime| runtime.api_key.clone()))
            .or_else(|| {
                crate::config::string(
                    &config,
                    &[
                        "providers",
                        reference.split('/').next().unwrap_or_default(),
                        "apiKey",
                    ],
                )
            })
            .or_else(environment_api_key)
    };
    let endpoint = options.endpoint.clone().or_else(|| {
        options
            .embedding
            .as_ref()
            .and_then(|embedding| embedding.endpoint.clone())
            .or_else(|| existing_runtime.and_then(|runtime| runtime.endpoint.clone()))
    });
    let endpoint = if local {
        endpoint
    } else {
        let endpoint = crate::authorization::remote_endpoint(&reference, endpoint.as_deref())?;
        let root = resolve_root(options.root.as_deref())?;
        crate::authorization::require_with_targets(
            &root,
            &reference,
            &endpoint,
            options.allow_remote,
            &options.authorized_remote,
        )?;
        Some(endpoint)
    };
    let device = if local {
        crate::config::runtime_device(
            &config,
            &reference,
            options.runtime_device.or(options.device).or_else(|| {
                options
                    .embedding
                    .as_ref()
                    .map(|e| e.device)
                    .filter(|device| *device != Device::Auto)
            }),
            existing_runtime.and_then(|runtime| runtime.device),
        )?
    } else {
        None
    };
    models
        .acquire(ModelRuntimeRequest::new(
            reference.clone(),
            ModelConfig {
                api_key,
                endpoint,
                cache_dir: crate::config::model_cache(
                    &config,
                    options
                        .model_cache
                        .clone()
                        .or_else(|| options.embedding.as_ref().and_then(|e| e.cache_dir.clone())),
                    existing_runtime.and_then(|runtime| runtime.cache_dir.clone()),
                ),
                device,
            },
            options.embedding_concurrency,
        ))
        .map_err(ModelError::into_engine_error)
}

/// Reports whether the configured local model can decide this build's model,
/// i.e. the build is implicit and nothing explicit or stored already wins.
fn needs_implicit_fallback(implicit: bool, has_requested: bool, has_stored: bool) -> bool {
    implicit && !has_requested && !has_stored
}

pub(crate) fn embedding_reference(
    existing: Option<&WorkspaceManifest>,
    requested: Option<&EmbeddingModelSpec>,
    language_default: Option<&'static str>,
    implicit: bool,
    suppress_environment: bool,
) -> Result<String, EngineError> {
    let config = crate::config::read()?;
    // An implicit build may use a configured local model, but never a remote
    // one, and a stored model still outranks both. Resolving it as a fallback
    // rather than as an explicit request keeps a refresh of an existing index
    // on its stored model. Only resolve it when it can decide the outcome, so
    // an unusable configured value cannot fail a request that names its model.
    let has_stored = existing.and_then(|manifest| manifest.embedding()).is_some();
    let implicit_local = if needs_implicit_fallback(implicit, requested.is_some(), has_stored) {
        if suppress_environment {
            crate::config::implicit_local_default()?
        } else {
            crate::config::implicit_embedding_override()?
        }
    } else {
        None
    };
    resolve_embedding_reference(ResolveEmbeddingReferenceOptions {
        explicit: requested.map(|embedding| embedding.reference.clone()),
        existing: existing
            .and_then(|manifest| manifest.embedding())
            .map(|embedding| embedding.model.reference()),
        global_default: (!implicit)
            .then(|| crate::config::string(&config, &["defaults", "embedding"]))
            .flatten(),
        // An empty map suppresses ZVEC_GREP_EMBEDDING for implicit builds,
        // which must never select a remote model on their own.
        environment: implicit.then(std::collections::HashMap::new),
        fallback: Some(
            implicit_local
                .or_else(|| language_default.map(str::to_owned))
                .unwrap_or_else(|| DEFAULT_LOCAL_EMBEDDING.to_owned()),
        ),
    })
    .map_err(ModelError::into_engine_error)?
    .ok_or_else(|| EngineError::internal("default embedding model is not configured"))
}

pub(in crate::pipelines) fn environment_api_key() -> Option<String> {
    ["ZVEC_GREP_API_KEY", "DASHSCOPE_API_KEY", "QWEN_API_KEY"]
        .into_iter()
        .find_map(|name| {
            env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
}

pub(in crate::pipelines) fn assert_embedding_compatible(
    existing: Option<&WorkspaceManifest>,
    model: &ModelRuntimeLease,
) -> Result<(), EngineError> {
    let Some(existing) = existing.filter(|manifest| is_indexed(manifest)) else {
        return Ok(());
    };
    let schema = existing
        .embeddings()
        .iter()
        .find(|schema| schema.model.reference() == model.info().model.reference())
        .ok_or_else(|| {
            EngineError::invalid_argument(
                "model is not part of the workspace index; rebuild the index",
            )
        })?;
    // The capsule comes from the construction-time info; the identity is
    // checked against the runtime's live execution path, because a loaded
    // session that fell back to another provider must be compared by what it
    // now uses, not by what its device requested.
    schema.ensure_model_compatible(model.info())?;
    let current = model
        .backend_identity()
        .or_else(|| model.info().backend_identity());
    schema.ensure_backend_compatible(current.as_ref())
}

fn resolve_scan(
    existing: Option<&WorkspaceManifest>,
    options: &IndexOptions,
) -> crate::domain::ScanRules {
    let mut scan = if options.reset_paths {
        crate::domain::ScanRules::default()
    } else {
        existing.map_or_else(crate::domain::ScanRules::default, |manifest| {
            manifest.workspace.scan.clone()
        })
    };
    options.scan.apply(&mut scan);
    scan
}

fn embedding_runtime(
    existing: Option<&WorkspaceManifest>,
    options: &IndexOptions,
    model: &ModelRuntimeLease,
) -> Result<ModelConfig, EngineError> {
    let current = existing
        .and_then(|manifest| {
            manifest
                .embedding_runtimes
                .get(&model.info().model.reference())
                .cloned()
        })
        .unwrap_or_default();
    let config = crate::config::read()?;
    if model.info().model.provider == "local" {
        let reference = format!(
            "{}/{}",
            model.info().model.provider,
            model.info().model.name
        );
        Ok(ModelConfig {
            cache_dir: crate::config::model_cache(
                &config,
                options
                    .model_cache
                    .clone()
                    .or_else(|| options.embedding.as_ref().and_then(|e| e.cache_dir.clone())),
                current.cache_dir,
            ),
            device: crate::config::runtime_device(
                &config,
                &reference,
                options.device.or_else(|| {
                    options
                        .embedding
                        .as_ref()
                        .map(|e| e.device)
                        .filter(|device| *device != Device::Auto)
                }),
                current.device,
            )?,
            ..ModelConfig::default()
        })
    } else {
        Ok(ModelConfig {
            api_key: current.api_key,
            endpoint: model.info().model.endpoint.clone().or(current.endpoint),
            device: None,
            cache_dir: None,
        })
    }
}

pub(in crate::pipelines) fn is_indexed(manifest: &WorkspaceManifest) -> bool {
    manifest.workspace.index_enabled() && manifest.index_version.is_some()
}

fn validate_workspace_root(root: &Path) -> Result<(), EngineError> {
    if !root.is_dir() {
        return Err(EngineError::invalid_argument(format!(
            "workspace root must be an existing directory: {}",
            root.display()
        )));
    }
    Ok(())
}

fn workspace_index_location_from_option(
    root: Option<&Path>,
) -> Result<WorkspaceIndexLocation, EngineError> {
    workspace_index_location(&resolve_root(root)?)
}

fn resolve_root(root: Option<&Path>) -> Result<PathBuf, EngineError> {
    let root = root
        .map_or_else(env::current_dir, |root| Ok(root.to_path_buf()))
        .map_err(|error| EngineError::from_io("failed to resolve current directory", &error))?;
    if root.is_absolute() {
        Ok(root)
    } else {
        env::current_dir()
            .map(|current| current.join(root))
            .map_err(|error| EngineError::from_io("failed to resolve workspace root", &error))
    }
}

fn workspace_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace")
        .to_owned()
}

fn unindexed_info(location: WorkspaceIndexLocation, policy: WorkspaceIndexPolicy) -> InfoResult {
    InfoResult {
        compatibility: IndexCompatibility::Unbuilt,
        root: location.root,
        indexed: false,
        index_policy: policy,
        home: location.home,
        index_path: location.index_path,
        source: InfoSource::Unindexed,
        workspace_index: None,
        status: None,
        suggestion: Some("run zg --index to create a workspace index".to_owned()),
    }
}

fn workspace_suggestion(manifest: &WorkspaceManifest, indexed: bool) -> Option<String> {
    if manifest.workspace.index == IndexState::Disabled {
        Some("indexing is disabled for this workspace".to_owned())
    } else if !indexed {
        Some("workspace manifest exists but index storage is missing".to_owned())
    } else {
        None
    }
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().try_into().unwrap_or(u64::MAX)
        })
}

fn workspace_info(manifest: &WorkspaceManifest) -> WorkspaceIndexInfo {
    WorkspaceIndexInfo::from_workspace(&manifest.workspace, &manifest.path, manifest.index_version)
}

/// Find the old registration of a physically moved workspace. The recorded root
/// also recovers a registry rename committed before the local manifest update.
fn moved_registration(
    registry: &WorkspaceRegistry,
    manifest: &WorkspaceManifest,
) -> Result<Option<(String, PathBuf)>, EngineError> {
    moved_registration_for(
        registry,
        &manifest.workspace.name,
        &manifest.recorded_root,
        &manifest.workspace.root,
    )
}

fn moved_registration_for(
    registry: &WorkspaceRegistry,
    name: &str,
    recorded_root: &Path,
    current_root: &Path,
) -> Result<Option<(String, PathBuf)>, EngineError> {
    let absent = |root: &Path| {
        root.try_exists().map(|exists| !exists).map_err(|error| {
            EngineError::from_io(
                format!("inspect original workspace {}", root.display()),
                &error,
            )
        })
    };
    if recorded_root != current_root
        && absent(recorded_root)?
        && let Some(name) = registry.name_for_root(recorded_root)?
    {
        return Ok(Some((name, recorded_root.to_path_buf())));
    }
    if let Some(root) = registry.root_for_name(name)?
        && absent(&root)?
    {
        return Ok(Some((name.to_owned(), root)));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;
    use zg_host_native::{ScanRequest, TaskControl, WorkspaceScannerPort};

    use crate::{
        api::{
            index::{
                IndexOptions,
                options::{EmbeddingModelSpec, ScanRulesUpdate},
            },
            info::InfoOptions,
        },
        domain::{IndexState, Workspace, model::Device},
    };

    use super::{ModelRuntimeManager, WorkspaceIndexService};

    fn write_previous_index_version(
        home: &std::path::Path,
        manifest: &crate::workspace::manifest::WorkspaceManifest,
    ) {
        let mut previous = manifest.clone();
        previous.index_version = Some(super::CURRENT_INDEX_VERSION - 1);
        std::fs::write(
            home.join("manifest.json"),
            serde_json::to_vec(&previous).expect("previous manifest JSON"),
        )
        .expect("previous index metadata");
    }

    #[test]
    fn rejects_incompatible_index_versions() {
        crate::workspace::manifest::require_current_index_version(Some(
            super::CURRENT_INDEX_VERSION,
        ))
        .expect("current index");
        for version in [
            None,
            Some(0),
            Some(1),
            Some(super::CURRENT_INDEX_VERSION - 1),
            Some(super::CURRENT_INDEX_VERSION + 1),
            Some(u32::MAX),
        ] {
            let error = crate::workspace::manifest::require_current_index_version(version)
                .expect_err("incompatible index format");
            assert!(error.message().contains("rebuild the index"));
        }
    }

    #[test]
    fn selection_updates_preserve_omitted_settings_and_clear_explicit_values() {
        let directory = tempdir().expect("workspace");
        let mut manifest = crate::workspace::manifest::WorkspaceManifest::new(
            Workspace {
                name: "workspace".into(),
                root: directory.path().to_path_buf(),
                scan: crate::domain::ScanRules {
                    globs: vec!["*.rs".into()],
                    ..crate::domain::ScanRules::default()
                },
                index: IndexState::Uninitialized,
                created_epoch_ms: 0,
                updated_epoch_ms: 0,
            },
            directory.path().join(".zvec-grep"),
            None,
            std::collections::BTreeMap::new(),
        )
        .expect("manifest");
        manifest.workspace.scan.hidden = true;
        manifest.workspace.scan.max_depth = Some(3);
        let mut options = IndexOptions::default();
        assert_eq!(
            super::resolve_scan(Some(&manifest), &options),
            manifest.workspace.scan
        );
        options.scan.globs = Some(Vec::new());
        options.scan.hidden = Some(false);
        options.scan.max_depth = Some(None);
        let scan = super::resolve_scan(Some(&manifest), &options);
        assert!(scan.globs.is_empty());
        assert!(!scan.hidden);
        assert_eq!(scan.max_depth, None);
        options = IndexOptions {
            reset_paths: true,
            ..IndexOptions::default()
        };
        assert_eq!(
            super::resolve_scan(Some(&manifest), &options),
            crate::domain::ScanRules::default()
        );
    }

    #[tokio::test]
    async fn a_file_cannot_be_a_workspace_root() {
        let directory = tempdir().expect("workspace");
        let file = directory.path().join("file.txt");
        std::fs::write(&file, "text").expect("source file");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let error = service
            .index(
                &models,
                IndexOptions {
                    root: Some(file),
                    ..IndexOptions::default()
                },
            )
            .await
            .expect_err("workspace requires a directory");
        assert!(error.message().contains("existing directory"));
        assert_eq!(models.snapshot().cached_runtimes, 0);
    }

    #[tokio::test]
    async fn reopening_a_moved_workspace_uses_its_current_root() {
        let directory = tempdir().expect("temporary directory");
        let original = directory.path().join("original");
        let moved = directory.path().join("moved");
        std::fs::create_dir(&original).expect("workspace root");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        service
            .index(
                &models,
                IndexOptions {
                    root: Some(original.clone()),
                    scan: ScanRulesUpdate {
                        globs: Some(vec!["*.rs".into()]),
                        ..ScanRulesUpdate::default()
                    },
                    embedding: Some(EmbeddingModelSpec {
                        reference: "local/potion-code-16m-v2".into(),
                        revision: None,
                        cache_dir: None,
                        endpoint: None,
                        device: Device::Cpu,
                    }),
                    ..IndexOptions::default()
                },
            )
            .await
            .expect("empty workspace index");
        let before = super::read_workspace_manifest(&original.join(".zvec-grep"))
            .expect("manifest read")
            .expect("manifest");
        std::fs::rename(&original, &moved).expect("move workspace");
        let info = service
            .info(InfoOptions {
                root: Some(moved.clone()),
                include_status: true,
            })
            .await
            .expect("moved workspace info");
        let workspace = info.workspace_index.expect("workspace index");
        assert_eq!(workspace.name, before.workspace.name.as_str());
        assert_eq!(
            workspace.root,
            std::fs::canonicalize(&moved).expect("moved root")
        );
        assert_eq!(workspace.path, workspace.root.join(".zvec-grep"));
        assert_eq!(
            workspace.scan.globs,
            vec![crate::domain::GlobRule::from("*.rs")]
        );
        assert_eq!(info.status.expect("status").files_scanned, 0);
        service
            .index(
                &models,
                IndexOptions {
                    root: Some(moved.clone()),
                    ..IndexOptions::default()
                },
            )
            .await
            .expect("update moved workspace");
        let after = super::read_workspace_manifest(&moved.join(".zvec-grep"))
            .expect("manifest read")
            .expect("manifest");
        assert_eq!(after.workspace.name, before.workspace.name);
        assert_eq!(after.workspace.scan, before.workspace.scan);
        assert_eq!(after.workspace.root, moved);
        assert_eq!(
            service
                .registry()
                .expect("registry")
                .root_for_name(&before.workspace.name)
                .expect("registered move"),
            Some(std::fs::canonicalize(&moved).expect("moved root"))
        );
        models.close();
    }

    #[tokio::test]
    async fn dropping_a_missing_index_is_an_idempotent_no_op() {
        let directory = tempdir().expect("temporary directory");

        assert!(
            !WorkspaceIndexService::with_test_registry()
                .drop_index(&InfoOptions {
                    root: Some(directory.path().to_path_buf()),
                    include_status: false,
                })
                .await
                .expect("missing index should be an idempotent no-op")
        );
    }

    #[tokio::test]
    async fn dropping_orphaned_generation_storage_does_not_require_a_manifest() {
        let directory = tempdir().expect("workspace");
        let home = directory.path().join(".zvec-grep");
        let storage = home
            .join("generations")
            .join(uuid::Uuid::new_v4().to_string())
            .join("storage");
        std::fs::create_dir_all(&storage).expect("orphaned generation");
        let service = WorkspaceIndexService::with_test_registry();
        let options = InfoOptions {
            root: Some(directory.path().to_path_buf()),
            include_status: false,
        };
        assert!(!home.join("manifest.json").exists());
        assert!(!home.join("build.json").exists());
        assert!(
            service
                .drop_index(&options)
                .await
                .expect("drop orphaned storage")
        );
        assert!(!storage.exists());
        assert!(home.join("generations").is_dir());
        assert!(
            !service
                .drop_index(&options)
                .await
                .expect("empty container is not an index")
        );
    }

    #[tokio::test]
    async fn cancellation_discards_stage_and_normal_index_keeps_active_storage() {
        let directory = tempdir().expect("workspace");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let options = empty_index_options(directory.path());
        service
            .index(&models, options.clone())
            .await
            .expect("initial index");
        let home = directory.path().join(".zvec-grep");
        let active = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active");
        let signal = tokio_util::sync::CancellationToken::new();
        signal.cancel();
        service
            .index(
                &models,
                IndexOptions {
                    rebuild: true,
                    signal: Some(signal),
                    ..options.clone()
                },
            )
            .await
            .expect_err("cancelled rebuild");
        assert_eq!(
            super::read_workspace_manifest(&home).expect("manifest"),
            Some(active.clone())
        );
        assert!(active.storage_home().join("storage").exists());
        assert!(!super::has_build(&home));
        let refresh = crate::pipelines::indexed_search::service::refresh_options(
            &crate::api::context::ContextOptions::default(),
            directory.path().to_path_buf(),
        );
        service
            .index(&models, refresh)
            .await
            .expect("ordinary refresh updates active index");
        let updated = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active");
        assert_eq!(updated.storage_generation, active.storage_generation);
        assert_eq!(updated.workspace.name, active.workspace.name);
        assert!(active.storage_home().exists());
        service
            .index(
                &models,
                IndexOptions {
                    rebuild: true,
                    ..options
                },
            )
            .await
            .expect("fresh rebuild");
        let rebuilt = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active");
        assert_ne!(rebuilt.storage_generation, active.storage_generation);
        assert!(!active.storage_home().exists());
        models.close();
    }

    #[tokio::test]
    async fn search_device_override_does_not_persist_in_manifest() {
        let directory = tempdir().expect("workspace");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        service
            .index(&models, empty_index_options(directory.path()))
            .await
            .expect("initial index");
        let home = directory.path().join(".zvec-grep");
        let mut manifest = super::read_workspace_manifest(&home)
            .expect("read")
            .expect("manifest");
        for runtime in manifest.embedding_runtimes.values_mut() {
            runtime.device = Some(Device::Auto);
        }
        super::write_workspace_manifest(&home, &manifest).expect("save automatic device");
        let request = crate::api::context::ContextOptions {
            device: Some(Device::Cpu),
            ..Default::default()
        };
        let options = crate::pipelines::indexed_search::service::refresh_options(
            &request,
            directory.path().to_path_buf(),
        );
        service.index(&models, options).await.expect("refresh");
        let refreshed = super::read_workspace_manifest(&home)
            .expect("read")
            .expect("manifest");
        assert_eq!(refreshed.embedding_runtimes, manifest.embedding_runtimes);
        service
            .index(
                &models,
                IndexOptions {
                    root: Some(directory.path().to_path_buf()),
                    device: Some(Device::Cpu),
                    ..Default::default()
                },
            )
            .await
            .expect("explicit configuration update");
        let configured = super::read_workspace_manifest(&home)
            .expect("read")
            .expect("manifest");
        assert!(
            configured
                .embedding_runtimes
                .values()
                .all(|runtime| runtime.device == Some(Device::Cpu))
        );
        models.close();
    }

    #[tokio::test]
    async fn abandoned_rebuild_does_not_change_normal_index_settings_or_storage() {
        let directory = tempdir().expect("workspace");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let options = empty_index_options(directory.path());
        service
            .index(&models, options.clone())
            .await
            .expect("initial index");
        let home = directory.path().join(".zvec-grep");
        let active = super::read_workspace_manifest(&home)
            .expect("read")
            .expect("active");
        let mut target = active.clone();
        target.workspace.scan.globs.push("*.rs".into());
        target
            .embedding_runtimes
            .values_mut()
            .next()
            .expect("runtime")
            .endpoint = Some("https://abandoned.test/embeddings".into());
        let abandoned = super::prepare_build(target).expect("crashed build");
        std::fs::write(
            abandoned.target.storage_home().join("checkpoint"),
            "discard me",
        )
        .expect("checkpoint");
        service
            .index(&models, options.clone())
            .await
            .expect("ordinary index uses active settings");
        let updated = super::read_workspace_manifest(&home)
            .expect("read")
            .expect("active");
        assert_eq!(updated.storage_generation, active.storage_generation);
        assert_eq!(updated.workspace.scan, active.workspace.scan);
        assert_eq!(updated.embedding_runtimes, active.embedding_runtimes);
        assert!(!abandoned.target.storage_home().exists());
        assert!(!super::has_build(&home));
        models.close();
    }

    #[tokio::test]
    async fn explicit_rebuild_starts_over_even_after_recovering_a_publication() {
        let directory = tempdir().expect("workspace");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let options = empty_index_options(directory.path());
        service
            .index(&models, options.clone())
            .await
            .expect("initial index");
        let home = directory.path().join(".zvec-grep");
        let active = super::read_workspace_manifest(&home)
            .expect("read")
            .expect("active");
        let published = super::prepare_build(active.clone()).expect("build");
        std::fs::create_dir(published.target.storage_home().join("storage"))
            .expect("completed storage");
        super::write_workspace_manifest(&home, &published.target)
            .expect("commit before simulated crash");
        service
            .index(
                &models,
                IndexOptions {
                    rebuild: true,
                    ..options
                },
            )
            .await
            .expect("fresh explicit rebuild");
        let rebuilt = super::read_workspace_manifest(&home)
            .expect("read")
            .expect("active");
        assert_ne!(
            rebuilt.storage_generation,
            published.target.storage_generation
        );
        assert!(!active.storage_home().exists());
        assert!(!published.target.storage_home().exists());
        assert!(rebuilt.storage_home().join("storage").exists());
        assert!(!super::has_build(&home));
        models.close();
    }

    #[tokio::test]
    async fn failed_checkpoint_does_not_publish_an_incomplete_rebuild() {
        let directory = tempdir().expect("workspace");
        let mut service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let options = empty_index_options(directory.path());
        service
            .index(&models, options.clone())
            .await
            .expect("initial index");
        let home = directory.path().join(".zvec-grep");
        let active = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active");
        service.fail_index_completion = true;
        let error = service
            .index(
                &models,
                IndexOptions {
                    rebuild: true,
                    ..options
                },
            )
            .await
            .expect_err("checkpoint fails");
        assert!(error.message().contains("checkpoint"));
        assert_eq!(
            super::read_workspace_manifest(&home).expect("manifest"),
            Some(active.clone())
        );
        assert!(active.storage_home().join("storage").exists());
        assert!(!super::has_build(&home));
        // The unchanged active index does not need a query-driven refresh.
        let location = super::workspace_index_location(directory.path()).expect("location");
        assert!(
            !service
                .workspace_needs_refresh(
                    &location,
                    &super::LockWait::new(None, None).expect("wait")
                )
                .await
                .expect("refresh decision")
        );
        assert!(
            service
                .drop_index(&InfoOptions {
                    root: Some(directory.path().to_path_buf()),
                    include_status: false
                })
                .await
                .expect("drop incomplete build")
        );
        assert!(!super::has_build(&home));
        assert!(!active.storage_home().exists());
        models.close();
    }

    #[test]
    fn implicit_fallback_only_decides_when_nothing_else_does() {
        // The configured local model may decide an implicit build only when no
        // explicit model was requested and no index model is stored. This is
        // the arm that keeps a daemon refresh on its existing model.
        assert!(super::needs_implicit_fallback(true, false, false));
        assert!(!super::needs_implicit_fallback(false, false, false));
        assert!(
            !super::needs_implicit_fallback(true, true, false),
            "an explicit request wins"
        );
        assert!(
            !super::needs_implicit_fallback(true, false, true),
            "a stored index model wins"
        );
    }

    #[test]
    fn a_configured_default_only_suppresses_profiling_when_it_will_be_used() {
        assert!(!super::configured_default_wins(None, true));
        assert!(super::configured_default_wins(
            Some("local/all-minilm-l6-v2"),
            true
        ));
        assert!(super::configured_default_wins(
            Some("local/all-minilm-l6-v2"),
            false
        ));
        assert!(
            !super::configured_default_wins(Some("qwen/text-embedding-v4"), true),
            "an implicit build ignores a remote default and still profiles"
        );
        assert!(super::configured_default_wins(
            Some("qwen/text-embedding-v4"),
            false
        ));
    }

    struct PythonWorkspace;

    #[async_trait::async_trait]
    impl WorkspaceScannerPort for PythonWorkspace {
        async fn discover(
            &self,
            _request: &ScanRequest,
            _control: &TaskControl,
        ) -> Result<zg_host_native::ScanSnapshot, zg_host_native::HostError> {
            Ok(zg_host_native::ScanSnapshot {
                files: vec![zg_host_native::DiscoveredFile {
                    root: PathBuf::from("/repo"),
                    relative_path: PathBuf::from("src/task.py"),
                    size_bytes: 900,
                    modified_epoch_ms: None,
                    source_fingerprint: String::new(),
                }],
                ..zg_host_native::ScanSnapshot::default()
            })
        }

        async fn read_batch(
            &self,
            _request: &zg_host_native::ReadBatchRequest,
            _control: &TaskControl,
        ) -> Result<Vec<zg_host_native::SourceFile>, zg_host_native::HostError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn a_python_dominant_workspace_selects_e5_small() {
        // End-to-end scan -> profile -> model for a workspace whose only source
        // file is Python, without acquiring or downloading a model.
        let directory = tempdir().expect("profile root");
        let selection =
            crate::file_selection::GlobMatcher::new(directory.path(), &[]).expect("matcher");
        let options = IndexOptions {
            implicit: true,
            ..IndexOptions::default()
        };
        let scan = crate::domain::ScanRules::default();
        assert_eq!(
            super::language_model_from_scan(
                &PythonWorkspace,
                &options,
                directory.path(),
                &scan,
                &selection,
            )
            .await
            .expect("language model"),
            Some("local/multilingual-e5-small")
        );
    }

    #[test]
    fn profile_model_picks_the_dominant_language() {
        let directory = tempdir().expect("profile root");
        let selection =
            crate::file_selection::GlobMatcher::new(directory.path(), &[]).expect("matcher");
        let files = [
            (PathBuf::from("pkg/validate.py"), 900),
            (PathBuf::from("cmd/main.rs"), 100),
            (PathBuf::from("README.md"), 5_000),
        ];
        assert_eq!(
            super::profile_model(files, &selection),
            Some("local/multilingual-e5-small")
        );
    }

    #[test]
    fn an_unmapped_majority_language_falls_back_to_the_generalist() {
        let directory = tempdir().expect("profile root");
        let selection =
            crate::file_selection::GlobMatcher::new(directory.path(), &[]).expect("matcher");
        // C++ outweighs Python, and no mapping exists for C++: no language may
        // speak for the repository.
        let cpp_majority = [
            (PathBuf::from("src/engine.cpp"), 100_000),
            (PathBuf::from("tools/task.py"), 1_000),
        ];
        assert_eq!(super::profile_model(cpp_majority, &selection), None);
        // The same listing with the majority mapped still selects.
        let python_majority = [
            (PathBuf::from("src/engine.cpp"), 1_000),
            (PathBuf::from("tools/task.py"), 100_000),
        ];
        assert_eq!(
            super::profile_model(python_majority, &selection),
            Some("local/multilingual-e5-small")
        );
    }

    #[test]
    fn profile_model_ties_and_unmapped_files_fall_back() {
        let directory = tempdir().expect("profile root");
        let selection =
            crate::file_selection::GlobMatcher::new(directory.path(), &[]).expect("matcher");
        let tied = [(PathBuf::from("a.rs"), 500), (PathBuf::from("b.java"), 500)];
        assert_eq!(super::profile_model(tied, &selection), None);
        let unmapped = [(PathBuf::from("notes.txt"), 10_000)];
        assert_eq!(super::profile_model(unmapped, &selection), None);
        // None falls back to the generalist default.
        assert_eq!(super::DEFAULT_LOCAL_EMBEDDING, "local/gte-modernbert-base");
    }

    #[test]
    fn profile_model_respects_the_workspace_globs() {
        let directory = tempdir().expect("profile root");
        let selection = crate::file_selection::GlobMatcher::new(
            directory.path(),
            &[crate::domain::GlobRule {
                pattern: "!vendor/**".to_owned(),
                case_insensitive: false,
            }],
        )
        .expect("matcher");
        let files = [
            (PathBuf::from("vendor/generated.py"), 10_000),
            (PathBuf::from("cmd/main.rs"), 100),
        ];
        assert_eq!(
            super::profile_model(files, &selection),
            Some("local/jina-embeddings-v2-base-code")
        );
    }

    fn empty_index_options(root: &Path) -> IndexOptions {
        IndexOptions {
            root: Some(root.to_path_buf()),
            embedding: Some(EmbeddingModelSpec {
                reference: "local/potion-code-16m-v2".into(),
                revision: None,
                cache_dir: None,
                endpoint: None,
                device: Device::Cpu,
            }),
            ..IndexOptions::default()
        }
    }

    async fn assert_nested_index_keeps_parent(model: &str, rebuild: bool) {
        let directory = tempdir().expect("nested workspaces");
        let root = std::fs::canonicalize(directory.path()).expect("parent root");
        let child = root.join("sub");
        std::fs::create_dir(root.join("docs")).expect("parent sources");
        std::fs::create_dir_all(child.join("src")).expect("child sources");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let mut parent_options = empty_index_options(&root);
        parent_options.embedding.as_mut().expect("model").reference =
            "local/potion-retrieval-32m".into();
        parent_options.scan.globs = Some(vec!["docs/**".into()]);
        service
            .index(&models, parent_options)
            .await
            .expect("parent index");
        let parent_home = root.join(".zvec-grep");
        let parent_bytes =
            std::fs::read(parent_home.join("manifest.json")).expect("parent manifest");
        let parent = super::read_workspace_manifest(&parent_home)
            .expect("manifest read")
            .expect("parent manifest");
        assert_eq!(
            service
                .info(InfoOptions {
                    root: Some(child.clone()),
                    include_status: false
                })
                .await
                .expect("ancestor info")
                .root,
            root
        );

        let mut options = empty_index_options(&child);
        options.embedding.as_mut().expect("model").reference = model.into();
        options.scan.globs = Some(vec!["src/**".into()]);
        options.rebuild = rebuild;
        service
            .index(&models, options)
            .await
            .expect("independent child index");
        let child_home = child.join(".zvec-grep");
        let indexed = super::read_workspace_manifest(&child_home)
            .expect("child manifest read")
            .expect("child owns its manifest");
        assert_eq!(indexed.workspace.root, child);
        assert_eq!(
            indexed.embedding().expect("child model").model.reference(),
            model
        );
        assert_eq!(
            indexed.workspace.scan.globs,
            vec![crate::domain::GlobRule::from("src/**")]
        );
        assert_eq!(
            std::fs::read(parent_home.join("manifest.json")).expect("parent manifest"),
            parent_bytes
        );
        assert!(super::IndexStore::exists(&parent.storage_home()).expect("parent storage remains"));
        assert_eq!(
            service
                .info(InfoOptions {
                    root: Some(child.clone()),
                    include_status: true
                })
                .await
                .expect("child info")
                .root,
            child
        );

        let mut incompatible = empty_index_options(&child);
        incompatible.embedding.as_mut().expect("model").reference =
            if model == "local/potion-retrieval-32m" {
                "local/potion-code-16m-v2".into()
            } else {
                "local/potion-retrieval-32m".into()
            };
        service
            .index(&models, incompatible)
            .await
            .expect_err("existing child model still requires rebuild");
        models.close();
    }

    #[tokio::test]
    async fn nested_index_with_same_model_keeps_parent() {
        assert_nested_index_keeps_parent("local/potion-retrieval-32m", false).await;
    }

    #[tokio::test]
    async fn nested_index_with_different_model_does_not_require_rebuild() {
        assert_nested_index_keeps_parent("local/potion-code-16m-v2", false).await;
    }

    #[tokio::test]
    async fn nested_index_rebuild_keeps_parent() {
        assert_nested_index_keeps_parent("local/potion-code-16m-v2", true).await;
    }

    #[tokio::test]
    async fn explicit_drop_releases_a_deleted_source_directory_without_recreating_it() {
        let directory = tempdir().expect("workspace roots");
        let deleted = directory.path().join("deleted");
        let replacement = directory.path().join("replacement");
        std::fs::create_dir(&deleted).expect("original root");
        std::fs::create_dir(&replacement).expect("replacement root");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        service
            .index(
                &models,
                IndexOptions {
                    name: Some("shared".into()),
                    ..empty_index_options(&deleted)
                },
            )
            .await
            .expect("original workspace owns name");
        std::fs::remove_dir_all(&deleted).expect("source directory deleted externally");
        let options = InfoOptions {
            root: Some(deleted.clone()),
            include_status: false,
        };
        assert!(
            service
                .drop_index(&options)
                .await
                .expect("drop deleted workspace")
        );
        assert!(!deleted.exists());
        assert!(
            !service
                .drop_index(&options)
                .await
                .expect("idempotent cleanup")
        );
        let name = "shared".to_owned();
        let registry = service.registry().expect("registry");
        assert_eq!(registry.root_for_name(&name).expect("released name"), None);
        service
            .index(
                &models,
                IndexOptions {
                    name: Some(name.clone()),
                    ..empty_index_options(&replacement)
                },
            )
            .await
            .expect("replacement workspace can reuse name");
        assert_eq!(
            registry.root_for_name(&name).expect("replacement owner"),
            Some(std::fs::canonicalize(&replacement).expect("replacement root"))
        );
        models.close();
    }

    #[tokio::test]
    async fn dropping_a_moved_workspace_releases_its_old_registration_before_reindexing() {
        let directory = tempdir().expect("workspace roots");
        let original = directory.path().join("original");
        let moved = directory.path().join("moved");
        let replacement = directory.path().join("replacement");
        std::fs::create_dir(&original).expect("original root");
        std::fs::create_dir(&replacement).expect("replacement root");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        service
            .index(
                &models,
                IndexOptions {
                    name: Some("shared".into()),
                    ..empty_index_options(&original)
                },
            )
            .await
            .expect("original workspace owns name");
        std::fs::rename(&original, &moved).expect("move without reopening index");
        assert!(
            service
                .drop_index(&InfoOptions {
                    root: Some(moved.clone()),
                    include_status: false,
                })
                .await
                .expect("drop moved workspace")
        );
        let name = "shared".to_owned();
        let registry = service.registry().expect("registry");
        assert_eq!(registry.root_for_name(&name).expect("released name"), None);
        assert!(!moved.join(".zvec-grep/manifest.json").exists());

        service
            .index(
                &models,
                IndexOptions {
                    name: Some("shared".into()),
                    ..empty_index_options(&replacement)
                },
            )
            .await
            .expect("another root can reuse the dropped name");
        assert_eq!(
            registry.root_for_name(&name).expect("new owner"),
            Some(std::fs::canonicalize(&replacement).expect("replacement root"))
        );
        models.close();
    }

    #[tokio::test]
    async fn copied_pending_first_build_cannot_steal_the_original_name() {
        let directory = tempdir().expect("workspace roots");
        let original = directory.path().join("original");
        let copy = directory.path().join("copy");
        std::fs::create_dir(&original).expect("original root");
        std::fs::create_dir_all(copy.join(".zvec-grep")).expect("copy home");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        service
            .index(
                &models,
                IndexOptions {
                    name: Some("shared".into()),
                    ..empty_index_options(&original)
                },
            )
            .await
            .expect("create workspace fixture");
        let home = original.join(".zvec-grep");
        let manifest = super::read_workspace_manifest(&home)
            .expect("read")
            .expect("manifest");
        super::prepare_build(manifest).expect("simulate crashed first build");
        std::fs::remove_file(home.join("manifest.json")).expect("unpublished fixture");
        let original_build = original.join(".zvec-grep/build.json");
        let original_bytes = std::fs::read(&original_build).expect("original pending build");
        assert!(!original.join(".zvec-grep/manifest.json").exists());
        std::fs::copy(&original_build, copy.join(".zvec-grep/build.json"))
            .expect("copy pending metadata");
        let error = service
            .index(
                &models,
                IndexOptions {
                    root: Some(copy.clone()),
                    ..IndexOptions::default()
                },
            )
            .await
            .expect_err("copy cannot claim the original reservation");
        assert!(error.message().contains("already registered"));
        let registry = service.registry().expect("registry");
        assert_eq!(
            registry.root_for_name("shared").expect("original owner"),
            Some(std::fs::canonicalize(&original).expect("original root"))
        );
        assert_eq!(
            std::fs::read(original_build).expect("original build unchanged"),
            original_bytes
        );
        assert_eq!(
            registry.name_for_root(&copy).expect("copy registration"),
            None
        );
        assert!(!copy.join(".zvec-grep/manifest.json").exists());
        models.close();
    }

    #[tokio::test]
    async fn invalid_names_are_rejected_before_workspace_mutation() {
        let directory = tempdir().expect("workspace");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let registry = service.registry().expect("registry");
        for name in ["", " project", "project/child", "project\nchild"] {
            let error = service
                .index(
                    &models,
                    IndexOptions {
                        name: Some(name.to_owned()),
                        ..empty_index_options(directory.path())
                    },
                )
                .await
                .expect_err("invalid workspace name");
            assert_eq!(error.code(), crate::EngineError::INVALID_ARGUMENT);
            assert!(error.message().contains("workspace name"));
            assert!(!directory.path().join(".zvec-grep").exists());
            assert_eq!(
                registry.name_for_root(directory.path()).expect("registry"),
                None
            );
        }
        assert_eq!(models.snapshot().cached_runtimes, 0);
    }

    #[tokio::test]
    async fn duplicate_names_are_rejected_before_model_acquisition() {
        let directory = tempdir().expect("workspace roots");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir(&first).expect("first workspace");
        std::fs::create_dir(&second).expect("second workspace");
        let service = WorkspaceIndexService::with_test_registry();
        let first_models = ModelRuntimeManager::new();
        service
            .index(
                &first_models,
                IndexOptions {
                    name: Some("shared".into()),
                    ..empty_index_options(&first)
                },
            )
            .await
            .expect("first workspace owns name");

        let second_models = ModelRuntimeManager::new();
        let error = service
            .clone()
            .index(
                &second_models,
                IndexOptions {
                    name: Some("shared".into()),
                    ..empty_index_options(&second)
                },
            )
            .await
            .expect_err("second workspace cannot claim name");
        assert!(error.message().contains("shared"));
        assert!(error.message().contains("already registered"));
        assert_eq!(second_models.snapshot().cached_runtimes, 0);
        assert!(!second.join(".zvec-grep/manifest.json").exists());
        assert!(!second.join(".zvec-grep/build.json").exists());
        assert_eq!(
            service
                .registry()
                .expect("registry")
                .root_for_name("shared")
                .expect("owner"),
            Some(std::fs::canonicalize(&first).expect("first root"))
        );
        first_models.close();
        second_models.close();
    }

    #[tokio::test]
    async fn explicit_rename_preserves_generation_storage() {
        let directory = tempdir().expect("workspace");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        service
            .index(
                &models,
                IndexOptions {
                    name: Some("before".into()),
                    ..empty_index_options(directory.path())
                },
            )
            .await
            .expect("initial index");
        let home = directory.path().join(".zvec-grep");
        let before = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active workspace");
        let marker = before.storage_home().join("storage/checkpoint-marker");
        std::fs::write(&marker, b"existing generation").expect("generation storage marker");
        service
            .index(
                &models,
                IndexOptions {
                    name: Some("after".into()),
                    root: Some(directory.path().to_path_buf()),
                    ..IndexOptions::default()
                },
            )
            .await
            .expect("rename workspace");
        let after = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("renamed workspace");
        assert_eq!(after.workspace.name.as_str(), "after");
        assert_eq!(after.storage_generation, before.storage_generation);
        assert_eq!(
            after.workspace.created_epoch_ms,
            before.workspace.created_epoch_ms
        );
        assert_eq!(
            std::fs::read(&marker).expect("retained generation storage"),
            b"existing generation"
        );
        let info = service
            .info(InfoOptions {
                root: Some(directory.path().to_path_buf()),
                include_status: false,
            })
            .await
            .expect("renamed info");
        assert_eq!(info.workspace_index.expect("workspace").name, "after");
        service
            .index(
                &models,
                IndexOptions {
                    root: Some(directory.path().to_path_buf()),
                    ..IndexOptions::default()
                },
            )
            .await
            .expect("update keeps new name");
        let updated = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("updated workspace");
        assert_eq!(updated.workspace.name, after.workspace.name);
        assert_eq!(updated.storage_generation, before.storage_generation);
        assert_eq!(
            std::fs::read(&marker).expect("retained generation storage"),
            b"existing generation"
        );
        assert_eq!(
            service
                .registry()
                .expect("registry")
                .root_for_name("before")
                .expect("released name"),
            None
        );
        models.close();
    }

    #[tokio::test]
    async fn registry_rename_is_replayed_after_manifest_update_is_interrupted() {
        let directory = tempdir().expect("workspace");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        service
            .index(
                &models,
                IndexOptions {
                    name: Some("before".into()),
                    ..empty_index_options(directory.path())
                },
            )
            .await
            .expect("initial index");
        let home = directory.path().join(".zvec-grep");
        let before = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active workspace");
        service
            .registry()
            .expect("registry")
            .rename(&before.workspace.name, "replayed", directory.path())
            .expect("commit registry rename before crash");
        let info = service
            .info(InfoOptions {
                root: Some(directory.path().to_path_buf()),
                include_status: false,
            })
            .await
            .expect("info reconciles name");
        assert_eq!(info.workspace_index.expect("workspace").name, "replayed");
        assert_eq!(
            super::read_workspace_manifest(&home).expect("read-only info retained manifest"),
            Some(before.clone())
        );
        service
            .index(
                &models,
                IndexOptions {
                    root: Some(directory.path().to_path_buf()),
                    ..IndexOptions::default()
                },
            )
            .await
            .expect("index replays rename");
        let after = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("reconciled workspace");
        assert_eq!(after.workspace.name.as_str(), "replayed");
        assert_eq!(after.storage_generation, before.storage_generation);
        models.close();
    }

    #[tokio::test]
    async fn moved_workspace_recovers_a_rename_committed_only_to_the_registry() {
        let directory = tempdir().expect("workspace roots");
        let original = directory.path().join("original");
        let moved = directory.path().join("moved");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let before = move_after_interrupted_rename(&service, &models, &original, &moved).await;
        let info = service
            .info(InfoOptions {
                root: Some(moved.clone()),
                include_status: false,
            })
            .await
            .expect("info resolves the authoritative name through the recorded old root");
        assert_eq!(info.workspace_index.expect("workspace").name, "after");
        let registry = service.registry().expect("registry");
        let new_name = "after".to_owned();
        assert_eq!(
            registry.root_for_name(&new_name).expect("read-only info"),
            Some(before.workspace.root.clone())
        );
        let home = moved.join(".zvec-grep");
        assert_eq!(
            super::read_workspace_manifest(&home)
                .expect("manifest")
                .expect("unchanged persisted name")
                .workspace
                .name,
            before.workspace.name
        );
        service
            .index(
                &models,
                IndexOptions {
                    root: Some(moved.clone()),
                    ..IndexOptions::default()
                },
            )
            .await
            .expect("index rebinds the renamed workspace after the move");
        let after = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("reconciled workspace");
        assert_eq!(after.workspace.name, new_name);
        assert_eq!(after.storage_generation, before.storage_generation);
        assert_eq!(
            registry
                .root_for_name(&new_name)
                .expect("moved registration"),
            Some(std::fs::canonicalize(&moved).expect("moved root"))
        );
        assert_eq!(
            registry
                .root_for_name(&before.workspace.name)
                .expect("old name remains released"),
            None
        );
        models.close();
    }

    #[tokio::test]
    async fn drop_after_interrupted_rename_and_move_releases_the_authoritative_name() {
        let directory = tempdir().expect("workspace roots");
        let original = directory.path().join("original");
        let moved = directory.path().join("moved");
        let replacement = directory.path().join("replacement");
        std::fs::create_dir(&replacement).expect("replacement root");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        move_after_interrupted_rename(&service, &models, &original, &moved).await;
        assert!(
            service
                .drop_index(&InfoOptions {
                    root: Some(moved),
                    include_status: false,
                })
                .await
                .expect("drop the moved workspace before rename replay")
        );
        let registry = service.registry().expect("registry");
        let new_name = "after".to_owned();
        assert_eq!(
            registry.root_for_name(&new_name).expect("released name"),
            None
        );
        assert_eq!(
            registry
                .name_for_root(&original)
                .expect("released old root"),
            None
        );
        service
            .index(
                &models,
                IndexOptions {
                    name: Some(new_name.clone()),
                    ..empty_index_options(&replacement)
                },
            )
            .await
            .expect("another workspace can reuse the authoritative name");
        assert_eq!(
            registry.root_for_name(&new_name).expect("new owner"),
            Some(std::fs::canonicalize(&replacement).expect("replacement root"))
        );
        models.close();
    }

    async fn move_after_interrupted_rename(
        service: &WorkspaceIndexService,
        models: &ModelRuntimeManager,
        original: &Path,
        moved: &Path,
    ) -> crate::workspace::manifest::WorkspaceManifest {
        std::fs::create_dir(original).expect("original root");
        let original = std::fs::canonicalize(original).expect("canonical original root");
        service
            .index(
                models,
                IndexOptions {
                    name: Some("before".into()),
                    ..empty_index_options(&original)
                },
            )
            .await
            .expect("initial index");
        let before = super::read_workspace_manifest(&original.join(".zvec-grep"))
            .expect("manifest")
            .expect("active workspace");
        service
            .registry()
            .expect("registry")
            .rename(&before.workspace.name, "after", &original)
            .expect("commit rename before manifest update is interrupted");
        std::fs::rename(original, moved).expect("move without replaying the rename");
        before
    }

    #[tokio::test]
    async fn drop_releases_name_reserved_before_initial_model_failure() {
        let directory = tempdir().expect("workspace");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let mut options = empty_index_options(directory.path());
        options.name = Some("reserved".into());
        options.embedding.as_mut().expect("model spec").revision = Some("unsupported".into());
        let error = service
            .index(&models, options)
            .await
            .expect_err("model acquisition fails");
        assert_eq!(error.code(), crate::EngineError::UNSUPPORTED);
        assert_eq!(models.snapshot().cached_runtimes, 0);
        let home = directory.path().join(".zvec-grep");
        assert!(!home.join("manifest.json").exists());
        assert!(!home.join("build.json").exists());
        let registry = service.registry().expect("registry");
        let name = "reserved".to_owned();
        assert_eq!(
            registry
                .name_for_root(directory.path())
                .expect("reservation"),
            Some(name.clone())
        );
        let info = InfoOptions {
            root: Some(directory.path().to_path_buf()),
            include_status: false,
        };
        assert!(
            service
                .drop_index(&info)
                .await
                .expect("drop reservation without manifest")
        );
        assert_eq!(registry.root_for_name(&name).expect("released name"), None);
        assert!(!service.drop_index(&info).await.expect("idempotent drop"));
        models.close();
    }

    #[tokio::test]
    async fn initial_build_cancellation_leaves_no_published_or_pending_index() {
        let directory = tempdir().expect("workspace");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let signal = tokio_util::sync::CancellationToken::new();
        signal.cancel();
        service
            .index(
                &models,
                IndexOptions {
                    signal: Some(signal),
                    ..empty_index_options(directory.path())
                },
            )
            .await
            .expect_err("initial build cancelled");
        let home = directory.path().join(".zvec-grep");
        assert!(!home.join("manifest.json").exists());
        assert!(!super::has_build(&home));
        assert!(!super::has_generation_storage(&home).expect("no abandoned storage"));
        service
            .index(&models, empty_index_options(directory.path()))
            .await
            .expect("fresh initial build");
        assert!(
            super::read_workspace_manifest(&home)
                .expect("manifest")
                .is_some()
        );
        models.close();
    }

    #[tokio::test]
    async fn composes_workspace_lifecycle_around_the_indexing_pipeline() {
        let directory = tempdir().expect("temporary directory");
        let sources = directory.path().join("sources");
        std::fs::create_dir(&sources).expect("source directory");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        let mut options = empty_index_options(directory.path());
        options.scan.globs = Some(vec!["sources/**".into()]);
        options.embedding.as_mut().expect("local model").cache_dir =
            Some(directory.path().join("model-cache"));
        let result = service
            .index(&models, options.clone())
            .await
            .expect("empty workspace should index");
        assert_eq!(result.files_scanned, 0);

        let info_options = InfoOptions {
            root: Some(directory.path().to_path_buf()),
            include_status: true,
        };
        let info = service
            .info(info_options.clone())
            .await
            .expect("workspace info");
        assert!(info.indexed);
        assert_eq!(info.status.expect("index status").files_stored, 0);
        assert!(info.workspace_index.is_some());
        let manifest = super::read_workspace_manifest(&info.home)
            .expect("manifest read")
            .expect("manifest");
        assert_eq!(
            manifest
                .embedding_runtimes
                .values()
                .next()
                .expect("runtime")
                .cache_dir,
            Some(directory.path().join("model-cache"))
        );
        let lease = crate::pipelines::indexed_search::service::acquire_search_model(
            &models,
            &manifest,
            None,
            &crate::api::context::ContextOptions::default(),
            directory.path(),
        )
        .expect("search model");
        assert_eq!(
            models.snapshot().cached_runtimes,
            1,
            "search reuses the configured model cache"
        );
        drop(lease);

        // An incompatible physical index requires an explicit rebuild.
        write_previous_index_version(&info.home, &manifest);
        let error = service
            .index(
                &models,
                IndexOptions {
                    root: Some(directory.path().to_path_buf()),
                    ..IndexOptions::default()
                },
            )
            .await
            .expect_err("previous index needs rebuild");
        assert!(error.message().contains("rebuild the index"));

        service
            .index(
                &models,
                IndexOptions {
                    rebuild: true,
                    ..options
                },
            )
            .await
            .expect("rebuild uses explicitly supplied configuration");
        let rebuilt = super::read_workspace_manifest(&info.home)
            .expect("manifest read")
            .expect("manifest");
        assert_eq!(rebuilt.index_version, Some(super::CURRENT_INDEX_VERSION));
        assert_eq!(rebuilt.workspace.root, manifest.workspace.root);
        assert_eq!(rebuilt.workspace.scan, manifest.workspace.scan);
        assert_eq!(rebuilt.embedding_runtimes, manifest.embedding_runtimes);
        assert_eq!(rebuilt.workspace.name, manifest.workspace.name);
        assert!(rebuilt.workspace.created_epoch_ms >= manifest.workspace.created_epoch_ms);
        assert_ne!(rebuilt.storage_generation, manifest.storage_generation);

        assert!(service.drop_index(&info_options).await.expect("drop index"));
        assert!(!super::IndexStore::exists(&rebuilt.storage_home()).expect("storage was deleted"));
        models.close();
    }

    async fn run_context_query(
        service: &WorkspaceIndexService,
        models: &ModelRuntimeManager,
        options: &crate::api::context::ContextOptions,
    ) -> Result<(), crate::EngineError> {
        crate::pipelines::indexed_search::service::context(service, models, options, None, false)
            .await
            .map(|_| ())
    }

    fn route_options(
        root: &Path,
        mode: crate::api::context::options::ContextRouteMode,
        auto_update: bool,
    ) -> crate::api::context::ContextOptions {
        crate::api::context::ContextOptions {
            root: Some(root.to_path_buf()),
            auto_update,
            routes: vec![crate::api::context::options::ContextRoute {
                mode,
                query: "orchard".into(),
            }],
            ..crate::api::context::ContextOptions::default()
        }
    }

    /// Simulate a generation recorded by a different (or older) embedding
    /// runtime by rewriting the manifest's backend identity on disk.
    fn rewrite_manifest_identity(
        manifest: &super::WorkspaceManifest,
        backend: Option<&str>,
        dtype: Option<&str>,
    ) {
        let embedding = crate::domain::model::EmbeddingModelInfo {
            backend: backend.map(str::to_owned),
            dtype: dtype.map(str::to_owned),
            ..manifest.embedding().expect("embedding").clone()
        };
        let mut rewritten = manifest.clone();
        rewritten.workspace.index =
            IndexState::Enabled(crate::domain::IndexDescriptor::single(embedding));
        let home = &manifest.path;
        let _writer = crate::workspace::lock::acquire_home_lock(
            home,
            crate::workspace::lock::LockMode::Write,
            "test.rewrite",
        )
        .expect("writer");
        super::write_workspace_manifest(home, &rewritten).expect("rewrite manifest");
    }

    #[tokio::test]
    async fn backend_identity_refuses_cross_backend_queries_until_rebuild() {
        let directory = tempdir().expect("workspace");
        let root = std::fs::canonicalize(directory.path()).expect("root");
        std::fs::write(root.join("notes.txt"), "orchard vines and orchard rows").expect("source");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        service
            .index(&models, empty_index_options(&root))
            .await
            .expect("index");
        let home = root.join(".zvec-grep");
        let manifest = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active");
        // The generation records the identity of the runtime that built it.
        let embedding = manifest.embedding().expect("embedding");
        assert_eq!(embedding.backend.as_deref(), Some("model2vec"));
        assert_eq!(embedding.dtype.as_deref(), Some("safetensors"));

        let vector = || {
            route_options(
                &root,
                crate::api::context::options::ContextRouteMode::Vector,
                false,
            )
        };
        let fts = || {
            route_options(
                &root,
                crate::api::context::options::ContextRouteMode::Fts,
                false,
            )
        };
        // Index-then-query on the same backend succeeds.
        run_context_query(&service, &models, &vector())
            .await
            .expect("same backend serves the index");

        // A generation recorded by a foreign runtime refuses vector queries.
        rewrite_manifest_identity(&manifest, Some("onnx-cpu"), Some("q8"));
        let error = run_context_query(&service, &models, &vector())
            .await
            .expect_err("cross-backend query must refuse");
        assert_eq!(error.code(), crate::EngineError::INDEX_BACKEND_MISMATCH);
        assert!(error.message().contains("onnx-cpu"), "{}", error.message());
        assert!(error.message().contains("model2vec"), "{}", error.message());
        assert!(error.message().contains("rebuild"), "{}", error.message());
        // FTS-only queries never acquire an embedding model, so a mismatched
        // generation keeps serving them instead of failing closed.
        run_context_query(&service, &models, &fts())
            .await
            .expect("fts stays usable on a foreign generation");
        // Incremental indexing also refuses; only an explicit rebuild recovers.
        service
            .index(&models, empty_index_options(&root))
            .await
            .expect_err("incremental index must refuse a foreign generation");

        // A legacy generation without a recorded identity fails closed.
        rewrite_manifest_identity(&manifest, None, None);
        let error = run_context_query(&service, &models, &vector())
            .await
            .expect_err("legacy identity must refuse");
        assert_eq!(error.code(), crate::EngineError::INDEX_BACKEND_MISMATCH);
        assert!(
            error.message().contains("built before backend recording"),
            "{}",
            error.message()
        );

        // Rebuilding on this runtime records its identity and serves queries.
        let mut rebuild = empty_index_options(&root);
        rebuild.rebuild = true;
        service
            .index(&models, rebuild)
            .await
            .expect("rebuild on this runtime");
        let reindexed = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("rebuilt index");
        assert_eq!(
            reindexed.embedding().expect("embedding").backend.as_deref(),
            Some("model2vec")
        );
        run_context_query(&service, &models, &vector())
            .await
            .expect("query after rebuild recovers");
        models.close();
    }

    #[tokio::test]
    async fn fts_only_auto_update_survives_an_incompatible_refresh() {
        let directory = tempdir().expect("workspace");
        let root = std::fs::canonicalize(directory.path()).expect("root");
        std::fs::write(root.join("notes.txt"), "orchard vines and orchard rows").expect("source");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        service
            .index(&models, empty_index_options(&root))
            .await
            .expect("index");
        let manifest = super::read_workspace_manifest(&root.join(".zvec-grep"))
            .expect("manifest")
            .expect("active");
        rewrite_manifest_identity(&manifest, Some("onnx-cpu"), Some("q8"));
        // A changed workspace makes an auto-update request attempt an
        // incremental refresh of the incompatible generation.
        std::fs::write(root.join("extra.txt"), "orchard rows extended").expect("changed source");
        // FTS-only retrieval survives the failed refresh and serves the stored
        // full-text results instead of failing the query.
        run_context_query(
            &service,
            &models,
            &route_options(
                &root,
                crate::api::context::options::ContextRouteMode::Fts,
                true,
            ),
        )
        .await
        .expect("auto-update fts serves results despite the incompatible generation");
        // Vector requests keep the hard refusal: they cannot mix embeddings
        // across runtimes, even when an automatic refresh is requested.
        let error = run_context_query(
            &service,
            &models,
            &route_options(
                &root,
                crate::api::context::options::ContextRouteMode::Vector,
                true,
            ),
        )
        .await
        .expect_err("auto-update vector query must still refuse");
        assert_eq!(error.code(), crate::EngineError::INDEX_BACKEND_MISMATCH);
        models.close();
    }

    #[tokio::test]
    async fn fts_only_auto_update_still_fails_on_other_refresh_errors() {
        let directory = tempdir().expect("workspace");
        let root = std::fs::canonicalize(directory.path()).expect("root");
        std::fs::write(root.join("notes.txt"), "orchard vines and orchard rows").expect("source");
        let service = WorkspaceIndexService::with_test_registry();
        let models = ModelRuntimeManager::new();
        service
            .index(&models, empty_index_options(&root))
            .await
            .expect("index");
        let home = root.join(".zvec-grep");
        let manifest = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active");
        // A changed workspace makes an auto-update request attempt an
        // incremental refresh. Rewriting the stored model to a reference the
        // catalog cannot resolve makes that refresh fail with an error that is
        // not the terminal backend mismatch, so it must propagate even for an
        // FTS-only request: only the identity mismatch is swallowable.
        std::fs::write(root.join("extra.txt"), "orchard rows extended").expect("changed source");
        let mut unknown_model = manifest.embedding().expect("embedding").clone();
        unknown_model.model.name = "does-not-exist".into();
        let mut rewritten = manifest.clone();
        rewritten.workspace.index =
            IndexState::Enabled(crate::domain::IndexDescriptor::single(unknown_model));
        {
            let _writer = crate::workspace::lock::acquire_home_lock(
                &home,
                crate::workspace::lock::LockMode::Write,
                "test.rewrite",
            )
            .expect("writer");
            super::write_workspace_manifest(&home, &rewritten).expect("rewrite manifest");
        }
        let error = run_context_query(
            &service,
            &models,
            &route_options(
                &root,
                crate::api::context::options::ContextRouteMode::Fts,
                true,
            ),
        )
        .await
        .expect_err("other refresh errors must fail the query");
        assert_ne!(error.code(), crate::EngineError::INDEX_BACKEND_MISMATCH);
        assert!(
            error.message().contains("does-not-exist"),
            "{}",
            error.message()
        );
        models.close();
    }

    /// A runtime whose construction-time identity names an accelerated
    /// provider but persistently executes on CPU: every generation it
    /// publishes records the live CPU identity.
    struct PersistentCpuFallbackStub {
        info: crate::domain::model::EmbeddingModelInfo,
    }

    #[async_trait::async_trait]
    impl crate::models::EmbeddingModel for PersistentCpuFallbackStub {
        fn info(&self) -> &crate::domain::model::EmbeddingModelInfo {
            &self.info
        }

        fn backend_identity(&self) -> Option<crate::domain::model::BackendIdentity> {
            Some(crate::domain::model::BackendIdentity {
                backend: "stub-cpu".to_owned(),
                dtype: "q8".to_owned(),
            })
        }

        async fn prepare(
            &self,
            _options: crate::models::EmbeddingPrepareOptions,
        ) -> Result<(), crate::models::ModelError> {
            Ok(())
        }

        async fn embed(
            &self,
            inputs: &[Vec<crate::domain::Content>],
            _options: crate::models::EmbeddingOptions,
        ) -> Result<crate::domain::model::EmbeddingResult, crate::models::ModelError> {
            Ok(crate::domain::model::EmbeddingResult {
                vectors: inputs
                    .iter()
                    .map(|_| vec![0.25; self.info.dimension])
                    .collect(),
                truncated: Vec::new(),
            })
        }
    }

    fn persistent_fallback_manager() -> ModelRuntimeManager {
        ModelRuntimeManager::with_factory(|_reference, _options, _compute| {
            Ok(std::sync::Arc::new(PersistentCpuFallbackStub {
                info: crate::domain::model::EmbeddingModelInfo {
                    model: crate::domain::model::ModelInfo {
                        provider: "local".into(),
                        name: "potion-code-16m-v2".into(),
                        endpoint: None,
                    },
                    dimension: 8,
                    metric: crate::domain::model::Metric::Cosine,
                    max_batch_size: 32,
                    max_input_tokens: Some(64),
                    max_image_bytes: None,
                    // The construction-time label names the accelerated
                    // provider the runtime never manages to use.
                    backend: Some("stub-accelerated".into()),
                    dtype: Some("q8".into()),
                },
            }))
        })
    }

    #[tokio::test]
    async fn a_persistently_falling_back_runtime_refreshes_incrementally() {
        let directory = tempdir().expect("workspace");
        let root = std::fs::canonicalize(directory.path()).expect("root");
        std::fs::write(root.join("notes.txt"), "orchard vines and orchard rows").expect("source");
        let service = WorkspaceIndexService::with_test_registry();
        let models = persistent_fallback_manager();
        service
            .index(&models, empty_index_options(&root))
            .await
            .expect("initial index");
        let home = root.join(".zvec-grep");
        let manifest = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active");
        // The published generation and its storage schema both record the
        // live CPU identity, not the construction-time accelerated label.
        assert_eq!(
            manifest.embedding().expect("embedding").backend.as_deref(),
            Some("stub-cpu")
        );
        let schema = std::fs::read_to_string(manifest.storage_home().join("storage/schema.json"))
            .expect("schema");
        assert!(schema.contains("stub-cpu"), "{schema}");
        assert!(!schema.contains("stub-accelerated"), "{schema}");

        // A changed workspace refreshes the same generation incrementally:
        // the refresh retries the identity check against the loaded runtime
        // instead of refusing the constructor-vs-live mismatch forever.
        std::fs::write(root.join("extra.txt"), "orchard rows extended").expect("changed source");
        service
            .index(&models, empty_index_options(&root))
            .await
            .expect("incremental refresh survives the persistent fallback");
        let refreshed = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("refreshed");
        assert_eq!(
            refreshed.storage_generation, manifest.storage_generation,
            "incremental refresh keeps the active generation"
        );

        // Vector queries are served by the same live identity.
        run_context_query(
            &service,
            &models,
            &route_options(
                &root,
                crate::api::context::options::ContextRouteMode::Vector,
                false,
            ),
        )
        .await
        .expect("query served by the fallback runtime");
        models.close();
    }

    /// A runtime that starts on its accelerated provider and falls back part
    /// way through a build, controlled by the test.
    struct MidRunFallbackStub {
        info: crate::domain::model::EmbeddingModelInfo,
        embeds: std::sync::Arc<std::sync::atomic::AtomicU32>,
        fallback_after: std::sync::Arc<std::sync::atomic::AtomicU32>,
    }

    #[async_trait::async_trait]
    impl crate::models::EmbeddingModel for MidRunFallbackStub {
        fn info(&self) -> &crate::domain::model::EmbeddingModelInfo {
            &self.info
        }

        fn backend_identity(&self) -> Option<crate::domain::model::BackendIdentity> {
            let fallen_back = self.embeds.load(std::sync::atomic::Ordering::Acquire)
                > self
                    .fallback_after
                    .load(std::sync::atomic::Ordering::Acquire);
            Some(crate::domain::model::BackendIdentity {
                backend: if fallen_back { "stub-cpu" } else { "stub-gpu" }.to_owned(),
                dtype: "q8".to_owned(),
            })
        }

        async fn prepare(
            &self,
            _options: crate::models::EmbeddingPrepareOptions,
        ) -> Result<(), crate::models::ModelError> {
            Ok(())
        }

        async fn embed(
            &self,
            inputs: &[Vec<crate::domain::Content>],
            _options: crate::models::EmbeddingOptions,
        ) -> Result<crate::domain::model::EmbeddingResult, crate::models::ModelError> {
            self.embeds
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            Ok(crate::domain::model::EmbeddingResult {
                vectors: inputs
                    .iter()
                    .map(|_| vec![0.25; self.info.dimension])
                    .collect(),
                truncated: Vec::new(),
            })
        }
    }

    /// Whether the named file exists in the generation and is recorded as
    /// fully indexed. `None` means the file has no record at all.
    fn indexed_file_state(storage_home: &Path, suffix: &str) -> Option<bool> {
        let storage = super::IndexStore::open(super::WorkspaceIndexStorageOptions::ReadOnly {
            storage_path: storage_home.to_path_buf(),
        })
        .expect("read storage");
        let files = storage.list_files().expect("file records");
        let state = files
            .iter()
            .find(|file| file.relative_path.to_path_buf().ends_with(suffix))
            .map(|file| {
                matches!(
                    file.index_status,
                    crate::domain::FileIndexStatus::Indexed { .. }
                )
            });
        storage.close().expect("close storage");
        state
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one scenario spans build, refused incremental, and recovery"
    )]
    async fn an_incremental_mid_run_fallback_refuses_instead_of_relabeling() {
        let directory = tempdir().expect("workspace");
        let root = std::fs::canonicalize(directory.path()).expect("root");
        std::fs::write(root.join("notes.txt"), "orchard vines and orchard rows").expect("source");
        let embeds = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let fallback_after = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(u32::MAX));
        let manager = {
            let embeds = std::sync::Arc::clone(&embeds);
            let fallback_after = std::sync::Arc::clone(&fallback_after);
            ModelRuntimeManager::with_factory(move |_reference, _options, _compute| {
                Ok(std::sync::Arc::new(MidRunFallbackStub {
                    info: crate::domain::model::EmbeddingModelInfo {
                        model: crate::domain::model::ModelInfo {
                            provider: "local".into(),
                            name: "potion-code-16m-v2".into(),
                            endpoint: None,
                        },
                        dimension: 8,
                        metric: crate::domain::model::Metric::Cosine,
                        max_batch_size: 32,
                        max_input_tokens: Some(64),
                        max_image_bytes: None,
                        backend: Some("stub-gpu".into()),
                        dtype: Some("q8".into()),
                    },
                    embeds: std::sync::Arc::clone(&embeds),
                    fallback_after: std::sync::Arc::clone(&fallback_after),
                }))
            })
        };
        let service = WorkspaceIndexService::with_test_registry();
        service
            .index(&manager, empty_index_options(&root))
            .await
            .expect("initial gpu index");
        let home = root.join(".zvec-grep");
        let manifest = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active");
        assert_eq!(
            manifest.embedding().expect("embedding").backend.as_deref(),
            Some("stub-gpu")
        );

        // The provider falls back from the next embedded batch onward.
        fallback_after.store(
            embeds.load(std::sync::atomic::Ordering::Acquire),
            std::sync::atomic::Ordering::Release,
        );
        std::fs::write(root.join("extra.txt"), "orchard rows extended").expect("changed source");
        let error = service
            .index(&manager, empty_index_options(&root))
            .await
            .expect_err("incremental mid-run fallback must refuse");
        assert_eq!(error.code(), crate::EngineError::INDEX_BACKEND_MISMATCH);
        assert!(
            error.message().contains("the storage opened with")
                && error.message().contains("produced vectors with"),
            "{}",
            error.message()
        );
        assert!(
            error.message().contains("stub-gpu") && error.message().contains("stub-cpu"),
            "{}",
            error.message()
        );
        // The live generation keeps its recorded identity instead of being
        // relabeled over vectors from both runtimes.
        let unchanged = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("active");
        assert_eq!(
            unchanged.embedding().expect("embedding").backend.as_deref(),
            Some("stub-gpu")
        );
        let schema = std::fs::read_to_string(unchanged.storage_home().join("storage/schema.json"))
            .expect("schema");
        assert!(schema.contains("stub-gpu"), "{schema}");
        assert!(!schema.contains("stub-cpu"), "{schema}");
        // The refusal fired before the flipped batch was committed: the
        // changed file is absent or unfinished, so no fallback-runtime
        // vectors entered the generation.
        assert_ne!(
            indexed_file_state(&unchanged.storage_home(), "extra.txt"),
            Some(true),
            "the refused batch must not leave indexed fallback vectors"
        );
        // Once the provider recovers, the same file is re-embedded and
        // published on the recorded identity - there is nothing to launder,
        // because no fallback vectors were committed.
        let embeds_before_recovery = embeds.load(std::sync::atomic::Ordering::Acquire);
        fallback_after.store(u32::MAX, std::sync::atomic::Ordering::Release);
        service
            .index(&manager, empty_index_options(&root))
            .await
            .expect("recovered runtime completes the incremental update");
        assert!(
            embeds.load(std::sync::atomic::Ordering::Acquire) > embeds_before_recovery,
            "the changed file is re-embedded by the recovered runtime"
        );
        let recovered = super::read_workspace_manifest(&home)
            .expect("manifest")
            .expect("recovered");
        assert_eq!(
            recovered.embedding().expect("embedding").backend.as_deref(),
            Some("stub-gpu")
        );
        assert_eq!(
            indexed_file_state(&recovered.storage_home(), "extra.txt"),
            Some(true),
            "the recovered run commits the changed file on the gpu identity"
        );
        manager.close();
    }
}
