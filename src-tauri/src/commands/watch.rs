//! Drift detection triggers (SPEC 4.4, 8.5): a file watcher on every client
//! config file, project manifest folder and skill folder this machine can
//! write, debounced and coalesced into one [`TargetsChanged`] event. The
//! frontend refetches the pending plan, the pills and the effective views
//! on it; drift itself is still found by the planner, and never reverted.
//!
//! Folders are watched, not files, and not recursively: a file that does
//! not exist yet cannot be watched, and a recursive watch on a project
//! folder would report every build output. A target whose folder does not
//! exist yet is watched through its nearest existing ancestor, so creating
//! that folder re-derives the set. The store and state folders are watched
//! too, so a new project, client preference or skill client re-derives the
//! set; their changes alone send no event (the app's own changes already
//! refresh the views, and a refresh never writes, so it cannot loop).
//!
//! Events carry no payload: no path, no file content crosses IPC. Loadout's
//! own apply writes cause one extra, harmless refresh after the one the
//! change itself triggers.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use agent_loadout_core::adapters::ADAPTERS;
use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::layering::manifest::manifest_dir;
use agent_loadout_core::layering::{ProjectStatus, TrustStore, known_projects};
use agent_loadout_core::model::applied::Scope as TargetScope;
use agent_loadout_core::model::clients::ClientPrefs;
use agent_loadout_core::skills::link::{self, CLIENT_SKILLS};
use agent_loadout_core::store::Machine;
use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::Core;

/// Quiet time after the last change before the event goes out (SPEC 4.4:
/// 500 ms).
pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// Longest a steady stream of changes (a client rewriting its file in a
/// loop) holds the event back.
const MAX_DELAY: Duration = Duration::from_secs(2);

/// A watched target file or folder changed outside Loadout's views: the
/// pending plan, the pills and the effective views are stale.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct TargetsChanged;

/// What one watcher run looks at. All paths are absolute.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct WatchSet {
    /// Client config files (user, project and project-local).
    pub(crate) files: BTreeSet<PathBuf>,
    /// Folders whose entries are targets: manifest and skill folders.
    pub(crate) folders: BTreeSet<PathBuf>,
    /// Folders whose changes may change the set itself: the store, the
    /// machine overlays and the state folder.
    pub(crate) control: BTreeSet<PathBuf>,
}

/// What a file system event means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Hit {
    None,
    /// Re-derive the set only.
    Control,
    /// Re-derive the set and tell the frontend.
    Target,
}

impl WatchSet {
    /// Every target this machine can write: the user files of every client,
    /// the project and project-local files, manifest folder and skill
    /// folders of every known project, and the user skill folders. Errors
    /// (no store yet, a broken overlay) leave out what they hide.
    pub(crate) fn derive(core: &Core) -> Self {
        let env = &core.env;
        let fs = core.fs.as_ref();
        let state_dir = core.home.state_dir();
        let desired = core.open_store().ok().flatten().and_then(|store| {
            let machine = Machine::load(fs, &state_dir).ok().flatten();
            DesiredState::load(&store, machine.as_ref().map(|m| m.id.as_str())).ok()
        });
        let trust = TrustStore::load(fs, &state_dir).unwrap_or_default();
        let projects = known_projects(
            fs,
            env,
            desired.as_ref().and_then(|d| d.overlay.as_ref()),
            &trust,
        );
        let roots: Vec<&Path> = projects
            .iter()
            .filter(|project| !matches!(project.status, ProjectStatus::Missing))
            .map(|project| project.root.as_path())
            .collect();

        let mut set = Self::default();
        for spec in ADAPTERS {
            let prefs = desired
                .as_ref()
                .map_or_else(ClientPrefs::default, |d| d.prefs(spec.id));
            let user = spec.mcp_targets(env, fs, TargetScope::User, None, &prefs);
            set.files.extend(user.into_iter().flatten().map(|t| t.file));
            for root in &roots {
                for scope in [TargetScope::Project, TargetScope::ProjectLocal] {
                    let targets = spec.mcp_targets(env, fs, scope, Some(root), &prefs);
                    set.files
                        .extend(targets.into_iter().flatten().map(|t| t.file));
                }
            }
        }
        for entry in &CLIENT_SKILLS {
            let user = link::skills_dir(env, entry.client, TargetScope::User, None);
            set.folders.extend(user);
            for root in &roots {
                let dir = link::skills_dir(env, entry.client, TargetScope::Project, Some(root));
                set.folders.extend(dir);
            }
        }
        // Every known project, a missing folder included: creating it
        // brings its targets in.
        set.folders
            .extend(projects.iter().map(|project| manifest_dir(&project.root)));
        let store = core.home.store_dir();
        set.control
            .extend([store.join("machines"), store, state_dir]);
        set
    }

    /// The folders to watch: each target's folder (a file's parent, a
    /// folder itself), or its nearest existing ancestor.
    pub(crate) fn anchors(&self) -> BTreeSet<PathBuf> {
        let parents = self.files.iter().filter_map(|file| file.parent());
        let folders = self
            .folders
            .iter()
            .chain(&self.control)
            .map(PathBuf::as_path);
        parents
            .chain(folders)
            .filter_map(existing_ancestor)
            .collect()
    }

    fn classify(&self, event: &Event) -> Hit {
        let structural = match event.kind {
            EventKind::Access(_) => return Hit::None,
            EventKind::Modify(ModifyKind::Name(_))
            | EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Any
            | EventKind::Other => true,
            EventKind::Modify(_) => false,
        };
        // Events were dropped: anything may have changed.
        if event.need_rescan() {
            return Hit::Target;
        }
        event
            .paths
            .iter()
            .map(|path| self.classify_path(path, structural))
            .max()
            .unwrap_or(Hit::None)
    }

    /// What a change of `path` means. A `structural` change creates,
    /// removes or renames it; any other change of a folder is only its
    /// timestamp moving with its entries (reported for the subfolders of a
    /// watched folder on Windows), and the folder's own watch reports those.
    fn classify_path(&self, path: &Path, structural: bool) -> Hit {
        let in_folder =
            |set: &BTreeSet<PathBuf>| path.parent().is_some_and(|dir| set.contains(dir));
        // `path` is a watched folder, or a folder on the way to one of
        // `targets`.
        let on_the_way = |targets: &BTreeSet<PathBuf>| {
            structural && targets.iter().any(|target| target.starts_with(path))
        };
        if self.files.contains(path)
            || in_folder(&self.folders)
            || on_the_way(&self.files)
            || on_the_way(&self.folders)
        {
            Hit::Target
        } else if in_folder(&self.control) || on_the_way(&self.control) {
            Hit::Control
        } else {
            Hit::None
        }
    }
}

/// `path` itself when it is a folder, else its nearest ancestor that is.
fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.is_dir())
        .map(Path::to_path_buf)
}

enum Msg {
    Fs(notify::Result<Event>),
    Stop,
}

/// The running watcher; dropping it stops the watch thread.
pub struct DriftWatch {
    tx: Sender<Msg>,
    worker: Option<JoinHandle<()>>,
}

impl DriftWatch {
    /// Watches the targets of `core` and calls `on_change` once per burst of
    /// changes, [`DEBOUNCE`] after the last one. The first watch set is in
    /// place when this returns.
    pub fn start(core: Arc<Core>, on_change: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let events = tx.clone();
        let watcher = notify::recommended_watcher(move |event| {
            // The receiver is gone only while the watch stops.
            let _ = events.send(Msg::Fs(event));
        })
        .map_err(std::io::Error::other)?;
        let mut worker = Worker {
            core,
            watcher,
            set: WatchSet::default(),
            watched: BTreeSet::new(),
        };
        worker.rescan();
        let worker = std::thread::Builder::new()
            .name("loadout-drift-watch".to_owned())
            .spawn(move || worker.run(&rx, &on_change))?;
        Ok(Self {
            tx,
            worker: Some(worker),
        })
    }
}

impl Drop for DriftWatch {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Worker {
    core: Arc<Core>,
    watcher: RecommendedWatcher,
    set: WatchSet,
    watched: BTreeSet<PathBuf>,
}

impl Worker {
    fn run(mut self, rx: &Receiver<Msg>, on_change: &dyn Fn()) {
        let mut burst: Option<(Instant, Instant)> = None;
        let mut hit = Hit::None;
        loop {
            let msg = match burst {
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some((_, due)) => rx.recv_timeout(due.saturating_duration_since(Instant::now())),
            };
            match msg {
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(Msg::Fs(event)) => {
                    let this = match event {
                        Ok(event) => self.set.classify(&event),
                        // A watched folder went away or the backend failed:
                        // re-derive, and let the views check.
                        Err(_) => Hit::Target,
                    };
                    if this == Hit::None {
                        continue;
                    }
                    hit = hit.max(this);
                    let now = Instant::now();
                    let first = burst.map_or(now, |(first, _)| first);
                    burst = Some((first, (now + DEBOUNCE).min(first + MAX_DELAY)));
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.rescan();
                    if hit == Hit::Target {
                        // The views refetch on the event: plan again.
                        self.core.plan_cache.invalidate();
                        on_change();
                    }
                    burst = None;
                    hit = Hit::None;
                }
            }
        }
    }

    /// Re-derives the set and moves the watches to its anchors. A folder
    /// that cannot be watched now is tried again on the next rescan.
    fn rescan(&mut self) {
        self.set = WatchSet::derive(&self.core);
        let anchors = self.set.anchors();
        for gone in self.watched.difference(&anchors) {
            let _ = self.watcher.unwatch(gone);
        }
        self.watched.retain(|dir| anchors.contains(dir));
        for dir in anchors {
            if !self.watched.contains(&dir)
                && self
                    .watcher
                    .watch(&dir, RecursiveMode::NonRecursive)
                    .is_ok()
            {
                self.watched.insert(dir);
            }
        }
    }
}
