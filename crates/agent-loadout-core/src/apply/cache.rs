//! The pending-plan cache (#15): one full plan ([`PlanOptions::default`])
//! and the desired state it was planned from, reused while nothing it was
//! planned from changed.
//!
//! A plan reads the store's working tree, `state/` (ledger, trust, machine),
//! every client file it plans and the folders detection probes, all through
//! the [`Fs`]. While a plan is computed, a recording view of the file system
//! notes each of these reads with what it saw (existence, content hash,
//! folder listing). The cached plan is served only after every noted read
//! gives the same answer again, so a store commit, a trust change, a client
//! file edited by its client, an apply from the CLI or a new project
//! manifest each make the next request plan again. The project repository
//! state that git2 reads directly is noted through [`Fs::note_dependency`].
//!
//! What the file system does not show:
//! - [`PlanCache::invalidate`] drops the plan (the app calls it on the drift
//!   watcher's event);
//! - a plan that read secret values (a keyring entry, an inline value) is
//!   *volatile*: it is shared with the requests that waited for it, never
//!   kept for later ones, because a secret store can change without any
//!   file changing.
//!
//! Requests that arrive while a plan is computed wait for it instead of
//! planning in parallel (one focus refetch asks for the plan two or three
//! times at once).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::access::ProbeCache;
use crate::apply::ApplyEnv;
use crate::apply::exec::PlanInputs;
use crate::apply::plan::{self, Plan, PlanOptions};
use crate::apply::render::DesiredState;
use crate::error::CoreError;
use crate::fs::Fs;
use crate::secrets::scanner::SecretMemory;
use crate::store::Store;

/// A full plan and what it was planned from.
pub struct CachedPlan {
    pub plan: Plan,
    pub(crate) inputs: PlanInputs,
}

impl CachedPlan {
    /// The desired state of the store's working tree (empty without a
    /// store).
    pub fn desired(&self) -> &DesiredState {
        &self.inputs.desired
    }

    /// The secret memory the plan rendered with.
    pub fn memory(&self) -> Arc<SecretMemory> {
        self.inputs.memory.clone()
    }
}

impl std::fmt::Debug for CachedPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedPlan")
            .field("plan", &self.plan)
            .finish_non_exhaustive()
    }
}

/// Opens the store (`None` when there is none yet) over the file system it
/// is given.
pub type OpenStore<'a> = dyn for<'f> Fn(&'f dyn Fs) -> Result<Option<Store<'f>>, CoreError> + 'a;

/// See the module documentation. One per process (the app's `Core`).
#[derive(Default)]
pub struct PlanCache {
    state: Mutex<State>,
    /// Held while a plan is computed, so concurrent requests wait for it.
    computing: Mutex<()>,
    /// Write probes of the target files, per file fingerprint (#4).
    probes: ProbeCache,
}

#[derive(Default)]
struct State {
    /// Bumped by [`PlanCache::invalidate`].
    generation: u64,
    /// Plans computed so far; a request may share a volatile plan only if
    /// it finished after the request arrived.
    finished: u64,
    entry: Option<Entry>,
}

struct Entry {
    plan: Arc<CachedPlan>,
    generation: u64,
    /// The value of [`State::finished`] this plan was stored under.
    finished: u64,
    volatile: bool,
    reads: Vec<(Read, Seen)>,
}

impl std::fmt::Debug for PlanCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanCache").finish_non_exhaustive()
    }
}

impl PlanCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// A cache whose planner runs wait `budget` for their write probes
    /// (default [`crate::access::PROBE_BUDGET`]).
    pub fn with_probe_budget(budget: std::time::Duration) -> Self {
        Self {
            probes: ProbeCache::with_budget(budget),
            ..Self::default()
        }
    }

    /// Drops the cached plan; the next request plans again. A plan being
    /// computed now is returned to its requests but not kept.
    pub fn invalidate(&self) {
        let mut state = self.lock();
        state.generation += 1;
        state.entry = None;
        self.probes.clear();
    }

    /// The write probes of client files, kept per file fingerprint; the
    /// app's views probe files outside the plan (manifests, skill folders)
    /// through it too.
    pub fn probes(&self) -> &ProbeCache {
        &self.probes
    }

    /// The full plan of this machine: the cached one when everything it was
    /// planned from reads the same, else a new one. `open` opens the store
    /// (or returns `None` when there is none yet) over the file system it
    /// is given, so the store's reads are noted too.
    pub fn plan(
        &self,
        ax: &ApplyEnv<'_>,
        open: &OpenStore<'_>,
    ) -> Result<Arc<CachedPlan>, CoreError> {
        let arrived = self.lock().finished;
        if let Some(plan) = self.reusable(ax.fs, arrived, false) {
            return Ok(plan);
        }
        let _computing = self
            .computing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Another request may have planned while this one waited.
        if let Some(plan) = self.reusable(ax.fs, arrived, true) {
            return Ok(plan);
        }
        let generation = self.lock().generation;
        let recorder = Recorder {
            inner: ax.fs,
            reads: Mutex::new(HashMap::new()),
            order: Mutex::new(Vec::new()),
        };
        let provider_reads = ax.delivery.provider_reads();
        let recorded = ApplyEnv {
            fs: &recorder,
            ..*ax
        };
        let store = open(&recorder)?;
        let inputs = PlanInputs::load(store.as_ref(), &recorded)?;
        let mut cx = recorded.render_cx(inputs.memory.clone());
        cx.probes = Some(&self.probes);
        let plan = plan::plan(
            &cx,
            &inputs.desired,
            &inputs.ledger,
            &inputs.key,
            &PlanOptions::default(),
        )?;
        drop(store);
        let volatile = ax.delivery.provider_reads() != provider_reads;
        let cached = Arc::new(CachedPlan { plan, inputs });
        let reads = recorder.into_reads();
        let mut state = self.lock();
        state.finished += 1;
        if state.generation == generation {
            state.entry = Some(Entry {
                plan: cached.clone(),
                generation,
                finished: state.finished,
                volatile,
                reads,
            });
        }
        Ok(cached)
    }

    /// The cached plan if it may serve a request that arrived when
    /// [`State::finished`] was `arrived`: only when every read it noted
    /// gives the same answer now (a plan that started before a store commit
    /// read the store as it was, so it fails this even when it finished
    /// after the request arrived), and for a volatile plan only when it
    /// also finished after the request arrived and the request `waited`.
    fn reusable(&self, fs: &dyn Fs, arrived: u64, waited: bool) -> Option<Arc<CachedPlan>> {
        let (plan, reads) = {
            let state = self.lock();
            let entry = state.entry.as_ref()?;
            if entry.generation != state.generation {
                return None;
            }
            if entry.volatile && !(waited && entry.finished > arrived) {
                return None;
            }
            (entry.plan.clone(), entry.reads.clone())
        };
        reads
            .iter()
            .all(|(read, seen)| read.observe(fs) == *seen)
            .then_some(plan)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One read a plan made.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Read {
    Exists(PathBuf),
    IsDir(PathBuf),
    Bytes(PathBuf),
    Text(PathBuf),
    Dir(PathBuf),
    Canonical(PathBuf),
}

/// What a read saw: a flag, a content hash, a path, or an error kind.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Flag(bool),
    Hash(u64),
    Path(PathBuf),
    Failed(io::ErrorKind),
}

fn hash_of(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn seen<T>(result: &io::Result<T>, ok: impl FnOnce(&T) -> Seen) -> Seen {
    match result {
        Ok(value) => ok(value),
        Err(err) => Seen::Failed(err.kind()),
    }
}

impl Read {
    /// What this read gives now.
    fn observe(&self, fs: &dyn Fs) -> Seen {
        match self {
            Read::Exists(path) => Seen::Flag(fs.exists(path)),
            Read::IsDir(path) => Seen::Flag(fs.is_dir(path)),
            Read::Bytes(path) => seen(&fs.read(path), |bytes| Seen::Hash(hash_of(bytes))),
            Read::Text(path) => seen(&fs.read_to_string(path), |text| {
                Seen::Hash(hash_of(text.as_bytes()))
            }),
            Read::Dir(path) => seen(&fs.read_dir(path), |entries| {
                let mut hasher = DefaultHasher::new();
                entries.hash(&mut hasher);
                Seen::Hash(hasher.finish())
            }),
            Read::Canonical(path) => seen(&fs.canonicalize(path), |path| Seen::Path(path.clone())),
        }
    }
}

/// The file system as a plan sees it, noting every read and its answer
/// (the first one, when a path is read twice).
struct Recorder<'a> {
    inner: &'a dyn Fs,
    reads: Mutex<HashMap<Read, Seen>>,
    order: Mutex<Vec<Read>>,
}

impl Recorder<'_> {
    fn note(&self, read: Read, seen: Seen) {
        let mut reads = self
            .reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !reads.contains_key(&read) {
            reads.insert(read.clone(), seen);
            self.order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(read);
        }
    }

    /// The reads in the order they were made, so a check stops at the
    /// first that differs.
    fn into_reads(self) -> Vec<(Read, Seen)> {
        let mut reads = self
            .reads
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.order
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .into_iter()
            .filter_map(|read| {
                let seen = reads.remove(&read)?;
                Some((read, seen))
            })
            .collect()
    }
}

impl Fs for Recorder<'_> {
    fn root(&self) -> Option<&Path> {
        self.inner.root()
    }

    fn exists(&self, path: &Path) -> bool {
        let found = self.inner.exists(path);
        self.note(Read::Exists(path.to_path_buf()), Seen::Flag(found));
        found
    }

    fn is_dir(&self, path: &Path) -> bool {
        let found = self.inner.is_dir(path);
        self.note(Read::IsDir(path.to_path_buf()), Seen::Flag(found));
        found
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        let result = self.inner.read_to_string(path);
        let seen = seen(&result, |text| Seen::Hash(hash_of(text.as_bytes())));
        self.note(Read::Text(path.to_path_buf()), seen);
        result
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let result = self.inner.read(path);
        let seen = seen(&result, |bytes| Seen::Hash(hash_of(bytes)));
        self.note(Read::Bytes(path.to_path_buf()), seen);
        result
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write(path, contents)
    }

    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> io::Result<()> {
        self.inner.write_new(path, contents, like)
    }

    fn replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.replace(from, to)
    }

    fn write_private(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write_private(path, contents)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }

    fn create_private_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_private_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_file(path)
    }

    fn is_regular_file(&self, path: &Path) -> bool {
        self.inner.is_regular_file(path)
    }

    fn overwrite_with_zeros(&self, path: &Path) -> io::Result<()> {
        self.inner.overwrite_with_zeros(path)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_dir_all(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let result = self.inner.read_dir(path);
        let seen = seen(&result, |entries| {
            let mut hasher = DefaultHasher::new();
            entries.hash(&mut hasher);
            Seen::Hash(hasher.finish())
        });
        self.note(Read::Dir(path.to_path_buf()), seen);
        result
    }

    fn open_lock_file(&self, path: &Path) -> io::Result<std::fs::File> {
        self.inner.open_lock_file(path)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        let result = self.inner.canonicalize(path);
        let seen = seen(&result, |path| Seen::Path(path.clone()));
        self.note(Read::Canonical(path.to_path_buf()), seen);
        result
    }

    fn note_dependency(&self, path: &Path) {
        let seen = seen(&self.inner.read(path), |bytes| Seen::Hash(hash_of(bytes)));
        self.note(Read::Bytes(path.to_path_buf()), seen);
    }

    /// Not a read the plan is checked against: probe results are kept per
    /// file fingerprint in the cache's [`ProbeCache`] instead.
    fn write_probe(&self) -> Option<crate::access::WriteProbe> {
        self.inner.write_probe()
    }
}
