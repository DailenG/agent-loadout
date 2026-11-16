//! One-click rollback of an apply run (SPEC 8.7).
//!
//! An apply run is found by its `Loadout-Apply` trailer. Rolling it back
//! restores, in a new commit, every desired-state file the run changed to
//! the state this machine had applied before it, then applies that commit
//! through the normal planner, so ownership rules hold: entries the run
//! created are removed only when unchanged since, and foreign entries are
//! never touched. History is never rewritten; `auto/pre-rollback/<ts>`
//! marks the commit before the rollback.
//!
//! "Applied before it" is the nearest earlier apply commit of this machine,
//! or the initial commit. In instant mode, where every edit is its own apply
//! commit, that is the parent of the apply commit; in review mode it also
//! covers the edits the run applied, which the parent commit already holds.
//! Other machines' files and every `*.applied.toml` are left alone. Rolling
//! back a rollback restores the original.

use crate::apply::exec::{ApplyReport, ApplyRequest, apply_in};
use crate::apply::plan::PlanOptions;
use crate::apply::{ApplyEnv, backup};
use crate::error::{CoreError, ErrorCode};
use crate::model::applied::ApplyTrigger;
use crate::store::{CommitId, LogEntry, LogFilter, Machine, Operation, Store};

/// Which apply run to roll back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplySelector {
    /// This machine's latest apply run.
    Last,
    /// An apply id.
    Id(String),
}

impl ApplySelector {
    /// `last` or an apply id.
    pub fn parse(text: &str) -> Self {
        if text == "last" {
            Self::Last
        } else {
            Self::Id(text.to_owned())
        }
    }
}

/// What a rollback restores.
#[derive(Debug, Clone)]
pub struct RollbackPlan {
    pub apply_id: String,
    /// The commit that carries the run's trailer.
    pub apply_commit: CommitId,
    /// The commit whose desired state is restored.
    pub restore_from: CommitId,
    /// Store files restored (store-relative).
    pub files: Vec<String>,
    /// Restored files that commits after the run changed too.
    pub changed_since: Vec<String>,
    /// Whether the run is this machine's latest apply run.
    pub latest: bool,
}

impl RollbackPlan {
    /// Rolling back an older run over later changes to the same files
    /// needs an explicit confirmation; the latest run never asks.
    pub fn needs_confirmation(&self) -> bool {
        !self.latest && !self.changed_since.is_empty()
    }
}

/// The result of a rollback.
#[derive(Debug)]
pub struct RollbackReport {
    pub plan: RollbackPlan,
    /// The `auto/pre-rollback/<ts>` tag.
    pub tag: String,
    pub apply: ApplyReport,
    pub commit: Option<CommitId>,
}

/// This machine's apply commits, newest first.
pub fn apply_commits(store: &Store<'_>, machine_id: &str) -> Result<Vec<LogEntry>, CoreError> {
    Ok(store
        .log(&LogFilter::default())?
        .into_iter()
        .filter(|entry| {
            entry.trailers.apply.is_some()
                && entry
                    .trailers
                    .machine
                    .as_ref()
                    .is_some_and(|machine| machine.id == machine_id)
        })
        .collect())
}

/// Works out what rolling back `selector` restores. Read-only.
pub fn prepare(
    store: &Store<'_>,
    machine_id: &str,
    selector: &ApplySelector,
) -> Result<RollbackPlan, CoreError> {
    let applies = apply_commits(store, machine_id)?;
    let index = match selector {
        ApplySelector::Last => (!applies.is_empty()).then_some(0),
        ApplySelector::Id(id) => applies.iter().position(|entry| {
            entry
                .trailers
                .apply
                .as_ref()
                .is_some_and(|apply| apply.id.eq_ignore_ascii_case(id))
        }),
    };
    let Some(index) = index else {
        return Err(CoreError::new(
            ErrorCode::NotFound,
            match selector {
                ApplySelector::Last => "this machine has no apply run to roll back".to_owned(),
                ApplySelector::Id(id) => format!("no apply run {id:?} on this machine"),
            },
        ));
    };
    let run = &applies[index];
    let restore_from = match applies.get(index + 1) {
        Some(previous) => previous.id.clone(),
        None => initial_commit(store, &run.id)?,
    };
    let files: Vec<String> = store
        .changed_files(&restore_from, &run.id)?
        .into_iter()
        .filter(|file| desired_state_file(file, machine_id))
        .collect();
    let head = store.head()?;
    let at_run = store.revision(&run.id)?;
    let at_head = store.revision(&head)?;
    let mut changed_since = Vec::new();
    for file in &files {
        if at_run.read_bytes(file)? != at_head.read_bytes(file)? {
            changed_since.push(file.clone());
        }
    }
    Ok(RollbackPlan {
        apply_id: run.trailers.apply.as_ref().expect("filtered").id.clone(),
        apply_commit: run.id.clone(),
        restore_from,
        files,
        changed_since,
        latest: index == 0,
    })
}

/// Rolls back an apply run: tag, restore, apply, one commit. Fails with
/// `confirmation_required` when [`RollbackPlan::needs_confirmation`] and
/// `confirmed` is false.
pub fn rollback(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    selector: &ApplySelector,
    confirmed: bool,
) -> Result<RollbackReport, CoreError> {
    let machine = Machine::load(ax.fs, &ax.home.state_dir())?.ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            "this machine has no apply run to roll back",
        )
    })?;
    let plan = prepare(store, &machine.id, selector)?;
    if plan.needs_confirmation() && !confirmed {
        return Err(CoreError::new(
            ErrorCode::ConfirmationRequired,
            format!(
                "later commits changed {} since apply {}; confirm to roll back over them",
                plan.changed_since.join(", "),
                plan.apply_id
            ),
        ));
    }
    let tag = free_tag(
        store,
        &format!("auto/pre-rollback/{}", backup::timestamp(ax.env.now())),
    );
    store.create_tag(&tag, &store.head()?)?;

    let mut operation = Operation::new(
        "apply.rollback",
        format!("rollback: apply {}", plan.apply_id),
    );
    for file in &plan.files {
        if let Some(entity) = entity_of(file) {
            operation = operation.entity(entity);
        }
    }
    let request = ApplyRequest {
        options: PlanOptions::default(),
        trigger: ApplyTrigger::Rollback,
    };
    let committed = store.transaction(operation, |tx| {
        let source = tx.store().revision(&plan.restore_from)?;
        let mut contents = Vec::with_capacity(plan.files.len());
        for file in &plan.files {
            contents.push((file.clone(), source.read_bytes(file)?));
        }
        drop(source);
        for (file, content) in contents {
            match content {
                Some(bytes) => tx.write(&file, bytes)?,
                None => tx.remove(&file)?,
            }
        }
        apply_in(tx, ax, &request)
    })?;
    Ok(RollbackReport {
        plan,
        tag,
        apply: committed.value,
        commit: committed.commit,
    })
}

/// The root commit reached by first parents from `from`.
fn initial_commit(store: &Store<'_>, from: &CommitId) -> Result<CommitId, CoreError> {
    let mut current = from.clone();
    while let Some(parent) = store.first_parent(&current)? {
        current = parent;
    }
    Ok(current)
}

/// Desired state of this machine: everything but apply records and other
/// machines' overlays.
fn desired_state_file(file: &str, machine_id: &str) -> bool {
    match file.strip_prefix("machines/") {
        Some(name) => name == format!("{machine_id}.toml"),
        None => true,
    }
}

/// The `Loadout-Entity` of a store file.
fn entity_of(file: &str) -> Option<String> {
    let mut parts = file.split('/');
    let first = parts.next()?;
    let second = parts.next();
    let stem = |name: &str| name.strip_suffix(".toml").map(str::to_owned);
    match (first, second) {
        ("servers", Some(name)) => stem(name).map(|id| format!("server/{id}")),
        ("skill-meta", Some(name)) => stem(name).map(|id| format!("skill/{id}")),
        ("skills", Some(id)) => Some(format!("skill/{id}")),
        ("projects", Some(id)) => Some(format!("project/{id}")),
        ("machines", Some(name)) => stem(name).map(|id| format!("machine/{id}")),
        (name, None) => stem(name),
        _ => None,
    }
}

/// `name`, or `name-2`, `name-3`, ... when taken.
fn free_tag(store: &Store<'_>, name: &str) -> String {
    if !store.tag_exists(name) {
        return name.to_owned();
    }
    (2..)
        .map(|n| format!("{name}-{n}"))
        .find(|candidate| !store.tag_exists(candidate))
        .expect("an unused suffix exists")
}
