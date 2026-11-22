//! `restore --from` and the app's "Restore from remote..." (SPEC 9.5,
//! 9.6, 11.9): a store from a folder remote's machine, a bare repository,
//! a mirror or a zip.
//!
//! - No store on this machine: the store is created from the source (its
//!   history, or the exported tree as one commit).
//! - A store of the same id: a repository merges like a pull (tagged
//!   `auto/pre-restore/<ts>`, conflicts recorded in `state/merge/`), an
//!   export restores as one new commit.
//! - Another store id: refused; restore into a fresh `LOADOUT_HOME`.
//!
//! Client files change only when `apply` is asked.

use crate::apply::{self, ApplyEnv, ApplyReport, ApplyRequest, PlanOptions};
use crate::brand;
use crate::error::{CoreError, ErrorCode};
use crate::model::applied::ApplyTrigger;
use crate::remote::folder::{self, MachineEntry, Source};
use crate::remote::sync_artifacts::Artifact;
use crate::store::merge::{MergeOutcome, PendingMerge, RecordedConflict, write_merge_state};
use crate::store::restore::{self, RestoreReport};
use crate::store::{CommitId, Store, bare_store_id, export_store_id};

/// An apply run and its commit.
pub type Applied = (ApplyReport, Option<CommitId>);

/// What a restore from a source did.
#[derive(Debug)]
pub enum FromOutcome {
    /// The store did not exist and was created from the source.
    Created {
        store_id: String,
        head: CommitId,
        apply: Option<Applied>,
    },
    /// The source's history merged into the existing store.
    Merged {
        outcome: MergeOutcome,
        /// `auto/pre-restore/<ts>`, when the source brought commits.
        tag: Option<String>,
        head: CommitId,
        apply: Option<Applied>,
    },
    /// An exported tree restored as one commit of the existing store.
    Restored(RestoreReport),
}

#[derive(Debug)]
pub struct FromReport {
    pub outcome: FromOutcome,
    /// Sync tool artifacts skipped in an exported tree, never read.
    pub artifacts: Vec<Artifact>,
}

impl FromReport {
    /// Conflicting fields of a merge (nothing merged then).
    pub fn conflicts(&self) -> usize {
        match &self.outcome {
            FromOutcome::Merged {
                outcome: MergeOutcome::Conflicts { conflicts, .. },
                ..
            } => conflicts.len(),
            _ => 0,
        }
    }

    pub fn apply(&self) -> Option<&ApplyReport> {
        match &self.outcome {
            FromOutcome::Created { apply, .. } | FromOutcome::Merged { apply, .. } => {
                apply.as_ref().map(|(report, _)| report)
            }
            FromOutcome::Restored(report) => report.apply.as_ref(),
        }
    }
}

fn run_apply(store: &Store<'_>, ax: &ApplyEnv<'_>) -> Result<Applied, CoreError> {
    let request = ApplyRequest {
        options: PlanOptions::default(),
        trigger: ApplyTrigger::Restore,
    };
    apply::run(store, ax, &request)
}

/// Restores from `source` (opened with [`folder::open_source`]); `machine`
/// is the folder remote machine it comes from, `label` names the source
/// in commit summaries and recorded merges.
pub fn restore_from(
    ax: &ApplyEnv<'_>,
    client: &str,
    source: &Source,
    machine: Option<&MachineEntry>,
    label: &str,
    apply: bool,
) -> Result<FromReport, CoreError> {
    let (env, fs, home) = (ax.env, ax.fs, ax.home);
    let mut artifacts = Vec::new();
    let (source_store_id, files) = match source {
        Source::Repo { bare, head, .. } => (bare_store_id(bare, head), None),
        export => {
            let export = folder::export_files(fs, export)?;
            artifacts = export.artifacts;
            (export_store_id(&export.files), Some(export.files))
        }
    };
    let Some(store) = Store::open(env, fs, home, client)? else {
        let store = match (source, files) {
            (Source::Repo { bare, head, tags }, _) => {
                Store::init_from_bare(env, fs, home, client, bare, head, tags)?
            }
            (_, Some(files)) => Store::init_from_files(
                env,
                fs,
                home,
                client,
                files,
                &format!("restore: from {label}"),
            )?,
            (_, None) => unreachable!("exports have files"),
        };
        let head = store.head()?;
        let apply = apply.then(|| run_apply(&store, ax)).transpose()?;
        return Ok(FromReport {
            outcome: FromOutcome::Created {
                store_id: store.store_id().to_owned(),
                head,
                apply,
            },
            artifacts,
        });
    };

    if source_store_id.as_deref() != Some(store.store_id()) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "the source holds another store ({}) than this machine's ({}); restore into a \
                 fresh {} instead",
                source_store_id.as_deref().unwrap_or("unknown"),
                store.store_id(),
                brand::HOME_ENV_VAR
            ),
        ));
    }
    let outcome = match (source, files) {
        (Source::Repo { bare, head, tags }, _) => {
            let mut ids = vec![head.clone()];
            ids.extend(tags.values().cloned());
            store.import_history(bare, &ids, tags)?;
            let incoming = CommitId::from_text(head)?;
            let tag = (!store.contains(&incoming))
                .then(|| store.create_auto_tag("pre-restore"))
                .transpose()?;
            let outcome = store.merge_incoming(&incoming, &format!("restore from {label}"))?;
            // Recorded like a pull's (SPEC 9.9), so the conflicts survive.
            let source_name = format!(
                "restore/{}",
                machine.map_or(label, |entry| entry.machine_id.as_str())
            );
            let pending: Vec<PendingMerge> = match &outcome {
                MergeOutcome::Conflicts { base, conflicts } => vec![PendingMerge {
                    source: source_name.clone(),
                    incoming: head.clone(),
                    base: base.as_ref().map(ToString::to_string),
                    conflicts: conflicts.iter().map(RecordedConflict::from).collect(),
                }],
                _ => Vec::new(),
            };
            write_merge_state(fs, store.state_dir(), &[source_name], &pending)?;
            let apply = (apply && !matches!(outcome, MergeOutcome::Conflicts { .. }))
                .then(|| run_apply(&store, ax))
                .transpose()?;
            FromOutcome::Merged {
                outcome,
                tag,
                head: store.head()?,
                apply,
            }
        }
        (_, Some(files)) => {
            FromOutcome::Restored(restore::restore_tree(&store, ax, &files, label, apply)?)
        }
        (_, None) => unreachable!("exports have files"),
    };
    Ok(FromReport { outcome, artifacts })
}
