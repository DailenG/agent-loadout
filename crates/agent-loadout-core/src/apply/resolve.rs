//! The drift resolver (SPEC 8.2, 8.6, 13.4): one entry that needs a
//! decision (`drifted`, `conflict`, `missing`, `collision`), shown as last
//! written, as in the file now and as desired, and resolved through the
//! planner's per-entry [`Decision`] in one store commit.

use crate::apply::exec::PlanInputs;
use crate::apply::ledger::EntryKind;
use crate::apply::plan::{
    self, Decision, EntryPlan, EntryState, OnConflict, PlanOptions, ProjectSelection, ScopeFilter,
    TargetFilter, TargetPlan,
};
use crate::apply::render::DesiredState;
use crate::apply::{ApplyEnv, ApplyReport, ApplyRequest, apply_in};
use crate::error::CoreError;
use crate::formats::NativeValue;
use crate::model::applied::{ApplyTrigger, Scope};
use crate::store::{CommitId, LogFilter, Machine, Operation, Store};

/// The three columns of the resolver, each rendered as the file writes the
/// entry ([`TargetPlan::snippet`]) and masked; `None` where there is
/// nothing (no file entry, nothing desired) or, for `last_written`, when it
/// cannot be recovered.
#[derive(Debug, Clone, Default)]
pub struct ThreeWay {
    pub last_written: Option<String>,
    pub current: Option<String>,
    pub desired: Option<String>,
}

/// The resolver's columns for `entry` of `target` (a target of the current
/// plan). What Loadout last wrote is not kept, only its keyed hash: for a
/// `drifted` entry it is the desired entry (the planner found their hashes
/// equal); otherwise it is rendered again from the store commit the
/// ledger names, and shown only when its hash matches the ledger's. A
/// collision was never written.
pub fn three_way(
    store: Option<&Store<'_>>,
    ax: &ApplyEnv<'_>,
    target: &TargetPlan,
    entry: &EntryPlan,
) -> Result<ThreeWay, CoreError> {
    let render = |value: &Option<NativeValue>| {
        value
            .as_ref()
            .and_then(|value| target.snippet(&entry.name, value))
    };
    let last_written = match entry.state {
        EntryState::Collision => None,
        EntryState::Drifted if entry.desired.is_some() => render(&entry.desired),
        _ => match store {
            Some(store) => recovered(store, ax, target, entry)?,
            None => None,
        },
    };
    Ok(ThreeWay {
        last_written,
        current: render(&entry.observed),
        desired: render(&entry.desired),
    })
}

/// What Loadout last wrote for `entry`, rendered again from the store and
/// shown only when the rendering hashes to the ledger's recorded hash. The
/// ledger names the store commit the apply started from; an apply inside
/// a store edit (instant mode) wrote what the next commit holds, so that
/// commit is tried first, then the named one.
fn recovered(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    target: &TargetPlan,
    entry: &EntryPlan,
) -> Result<Option<String>, CoreError> {
    let Some(client) = entry.client else {
        return Ok(None);
    };
    let inputs = PlanInputs::load(Some(store), ax)?;
    let file = target.file.to_string_lossy();
    let row = inputs.ledger.entries.iter().find(|row| {
        row.kind == EntryKind::McpEntry
            && row.file == file
            && row.container == target.pointer
            && row.key == entry.name
    });
    let Some((hash, commit)) =
        row.and_then(|row| row.written_hash.as_ref().zip(row.store_commit.as_ref()))
    else {
        return Ok(None);
    };
    // A commit that is gone (a rewritten store) recovers nothing.
    let Ok(base) = store.resolve(commit) else {
        return Ok(None);
    };
    let log = store.log(&LogFilter::default())?;
    let mut candidates = Vec::new();
    if let Some(index) = log.iter().position(|logged| logged.id == base)
        && index > 0
    {
        candidates.push(log[index - 1].id.clone());
    }
    candidates.push(base);

    let machine = Machine::load(ax.fs, &ax.home.state_dir())?;
    let options = PlanOptions {
        scope: match target.scope {
            Scope::User => ScopeFilter::System,
            Scope::Project | Scope::ProjectLocal => ScopeFilter::Project,
        },
        projects: match &target.project {
            Some(root) => ProjectSelection::Only(root.clone()),
            None => ProjectSelection::All,
        },
        targets: Some(vec![filter_of(target)]),
        ..PlanOptions::default()
    };
    let cx = ax.render_cx(inputs.memory.clone());
    for id in candidates {
        let revision = store.revision(&id)?;
        let Ok(desired) = DesiredState::load(&revision, machine.as_ref().map(|m| m.id.as_str()))
        else {
            continue;
        };
        let old = plan::plan(&cx, &desired, &inputs.ledger, &inputs.key, &options)?;
        for old_target in old
            .targets
            .iter()
            .filter(|old| old.pointer == target.pointer)
        {
            for old_entry in &old_target.entries {
                if old_entry.name != entry.name || old_entry.client != Some(client) {
                    continue;
                }
                let Some(value) = &old_entry.desired else {
                    continue;
                };
                let matches = plan::canonical(client, value)
                    .is_some_and(|canonical| inputs.key.hash(&canonical) == *hash);
                if matches {
                    // Masked by both plans: the values that commit renders
                    // inline and those the current plan knows.
                    return Ok(old_target
                        .snippet(&entry.name, value)
                        .map(|text| target.mask(&text)));
                }
            }
        }
    }
    Ok(None)
}

/// The filter that selects `target` alone.
pub fn filter_of(target: &TargetPlan) -> TargetFilter {
    TargetFilter {
        scope: target.scope,
        project: target.project.clone(),
        file: target.file.clone(),
    }
}

/// Resolves entry `name` of `client` in the target `filter` selects with
/// `resolution`, applying only that file, in one store transaction: a
/// commit when the store or a client file changes (an import or a release
/// edits the server, an overwrite writes the file), none otherwise.
/// Summaries: `adopt: <client>/<name> drift` for an import of a changed
/// entry (SPEC 8.6), else `resolve: <action> <client>/<name>`.
pub fn resolve(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    filter: TargetFilter,
    client: &str,
    name: &str,
    state: EntryState,
    resolution: OnConflict,
) -> Result<(ApplyReport, Option<CommitId>), CoreError> {
    let entry = format!("{client}/{name}");
    let summary = match (resolution, state) {
        (OnConflict::Import, EntryState::Missing) => format!("resolve: respect deletion {entry}"),
        (OnConflict::Import, EntryState::Collision) => format!("adopt: {entry} (import theirs)"),
        (OnConflict::Import, _) => format!("adopt: {entry} drift"),
        (other, _) => format!("resolve: {} {entry}", other.as_str()),
    };
    let request = ApplyRequest {
        options: PlanOptions {
            on_conflict: OnConflict::Ask,
            decisions: vec![Decision {
                client: client.to_owned(),
                name: name.to_owned(),
                resolution,
            }],
            targets: Some(vec![filter]),
            ..PlanOptions::default()
        },
        trigger: ApplyTrigger::Review,
    };
    let operation = Operation::new("entry.resolve", summary.clone());
    let committed = store.transaction(operation, |tx| {
        let report = apply_in(tx, ax, &request)?;
        // The apply names a file-only run `apply: ...`; the commit is the
        // resolution.
        tx.set_summary(summary);
        Ok(report)
    })?;
    Ok((committed.value, committed.commit))
}
