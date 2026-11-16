//! Shared fixture of the apply tests: a `TempHome` with a store in
//! `<root>/lh`, a `RootedFs`, and helpers to edit the store without applying
//! (review mode), apply, and inspect plans, files and the ledger.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::apply::ledger::Ledger;
use agent_loadout_core::apply::{
    self, ApplyEnv, ApplyReport, ApplyRequest, EntryPlan, Plan, PlanOptions,
};
use agent_loadout_core::model::TomlDocument;
use agent_loadout_core::model::applied::ApplyTrigger;
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::server::{Enable, Server, Transport};
use agent_loadout_core::paths::{LoadoutHome, loadout_home};
use agent_loadout_core::secrets::delivery::DeliveryEnv;
use agent_loadout_core::store::{
    CommitId, LogEntry, LogFilter, Operation, Store, Transaction, server_path,
};
use agent_loadout_core::{CoreError, Env, Os};
use agent_loadout_testkit::{MemKeyring, RootedFs, TempHome};

pub const CLIENT: &str = "loadout-test/0.0.0";

static NO_REGISTRY: NoRegistry = NoRegistry;

pub fn host_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

/// The fake `loadout` binary path of a temp home.
pub fn launcher(th: &TempHome) -> PathBuf {
    th.bin()
        .join(format!("loadout{}", std::env::consts::EXE_SUFFIX))
}

/// A stdio server enabled for `clients`.
pub fn stdio(id: &str, clients: &[&str], args: &[&str]) -> Server {
    let mut server = Server::new(id);
    server.body.transport = Some(Transport::Stdio);
    server.body.command = Some("npx".to_owned());
    server.body.args = Some(args.iter().map(|arg| (*arg).to_owned()).collect());
    if !clients.is_empty() {
        server.enable = Some(Enable {
            clients: Some(ClientSelection::List(
                clients.iter().map(|client| (*client).to_owned()).collect(),
            )),
            ..Enable::default()
        });
    }
    server
}

pub struct Fx {
    pub th: TempHome,
    pub env: Env,
    pub fs: RootedFs,
    pub home: LoadoutHome,
    /// The in-memory keyring every apply uses.
    pub keyring: Arc<MemKeyring>,
    /// No `op` on `PATH` (the env has no `PATH`): 1Password is not found.
    pub delivery: DeliveryEnv,
}

impl Fx {
    pub fn new() -> Self {
        let th = TempHome::new();
        let env = th.env_builder(host_os()).hostname("test-desk").build();
        let fs = th.fs();
        let home = loadout_home(&env, &fs, Some(&th.root().join("lh"))).unwrap();
        let keyring = Arc::new(MemKeyring::new());
        let delivery = DeliveryEnv::new(Some(launcher(&th)), keyring.clone());
        Self {
            th,
            env,
            fs,
            home,
            keyring,
            delivery,
        }
    }

    pub fn store(&self) -> Store<'_> {
        Store::open_or_init(&self.env, &self.fs, &self.home, CLIENT).unwrap()
    }

    pub fn ax(&self) -> ApplyEnv<'_> {
        ApplyEnv {
            env: &self.env,
            fs: &self.fs,
            home: &self.home,
            registry: &NO_REGISTRY,
            delivery: &self.delivery,
        }
    }

    /// The launcher path entries start (`<root>/bin/loadout`, never run).
    pub fn launcher(&self) -> String {
        launcher(&self.th).to_string_lossy().into_owned()
    }

    /// `~/.claude.json`.
    pub fn claude(&self) -> PathBuf {
        self.th.home().join(".claude.json")
    }

    /// `~/.omp/agent/mcp.json`.
    pub fn omp(&self) -> PathBuf {
        self.th.home().join(".omp").join("agent").join("mcp.json")
    }

    pub fn read(&self, path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    pub fn write(&self, path: &Path, text: &str) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    /// A store commit that does not apply (review mode).
    pub fn edit(
        &self,
        summary: &str,
        f: impl FnOnce(&mut Transaction<'_>) -> Result<(), CoreError>,
    ) -> Option<CommitId> {
        self.store()
            .transaction(Operation::new("test.edit", summary), f)
            .unwrap()
            .commit
    }

    /// Adds or replaces a server, without applying.
    pub fn put_server(&self, server: Server) -> Option<CommitId> {
        let relative = server_path(&server.id).unwrap();
        let text = TomlDocument::new(server).to_text().unwrap();
        self.edit("put server", |tx| tx.write(&relative, text))
    }

    pub fn remove_server(&self, id: &str) -> Option<CommitId> {
        self.edit("remove server", |tx| tx.remove_server(id))
    }

    /// A standalone apply run (review trigger).
    pub fn apply(&self) -> (ApplyReport, Option<CommitId>) {
        self.apply_with(PlanOptions::default())
    }

    pub fn apply_with(&self, options: PlanOptions) -> (ApplyReport, Option<CommitId>) {
        let request = ApplyRequest {
            options,
            trigger: ApplyTrigger::Review,
        };
        apply::run(&self.store(), &self.ax(), &request).unwrap()
    }

    /// An instant-mode edit: the change and its apply in one commit.
    pub fn instant(
        &self,
        summary: &str,
        f: impl FnOnce(&mut Transaction<'_>) -> Result<(), CoreError>,
    ) -> (ApplyReport, Option<CommitId>) {
        let store = self.store();
        let ax = self.ax();
        let committed = store
            .transaction(Operation::new("test.edit", summary), |tx| {
                f(tx)?;
                let request = ApplyRequest {
                    options: PlanOptions::default(),
                    trigger: ApplyTrigger::Instant,
                };
                apply::apply_in(tx, &ax, &request)
            })
            .unwrap();
        (committed.value, committed.commit)
    }

    pub fn plan(&self) -> Plan {
        self.plan_with(&PlanOptions::default())
    }

    pub fn plan_with(&self, options: &PlanOptions) -> Plan {
        let store = self.store();
        apply::preview(Some(&store), &self.ax(), options).unwrap()
    }

    pub fn ledger(&self) -> Ledger {
        Ledger::load(&self.fs, &self.home.state_dir()).unwrap()
    }

    pub fn log(&self) -> Vec<LogEntry> {
        self.store().log(&LogFilter::default()).unwrap()
    }

    pub fn commit_count(&self) -> usize {
        self.log().len()
    }

    pub fn machine_id(&self) -> String {
        agent_loadout_core::store::Machine::load(&self.fs, &self.home.state_dir())
            .unwrap()
            .unwrap()
            .id
    }
}

/// The plan of entry `name` in `file`.
pub fn entry<'a>(plan: &'a Plan, file: &Path, name: &str) -> &'a EntryPlan {
    plan.targets
        .iter()
        .filter(|target| target.file == file)
        .flat_map(|target| &target.entries)
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("no entry {name:?} for {} in {plan:#?}", file.display()))
}

/// The result entry `name` in `file` of an apply report.
pub fn result_entry<'a>(report: &'a ApplyReport, file: &Path, name: &str) -> &'a EntryPlan {
    report
        .targets
        .iter()
        .filter(|target| target.file == file)
        .flat_map(|target| &target.entries)
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("no entry {name:?} for {}", file.display()))
}

/// The JSON object of `mcpServers.<name>` in a file, if any.
pub fn json_entry(text: &str, name: &str) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    value.get("mcpServers")?.get(name).cloned()
}
