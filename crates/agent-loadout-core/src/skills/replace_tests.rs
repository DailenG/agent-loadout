//! The folder replacement of skills adopt (SPEC 12.5) with its file
//! operations made to fail: a trash on another drive, a copy that fails
//! half way, an entry that cannot be made. Each runs in its own temp
//! folder with the real file system; nothing outside it is read or written
//! (the store reads no git configuration).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use crate::adapters::detect::NoRegistry;
use crate::env::{Env, FixedClock, KnownFolder, Os};
use crate::error::{CoreError, ErrorCode};
use crate::fs::{Fs, RealFs};
use crate::layering::{Project, TrustStore};
use crate::model::applied::Scope;
use crate::paths::{LoadoutHome, loadout_home};
use crate::remote::git::SystemRunner;
use crate::skills::adopt;
use crate::skills::install::{InstallRequest, Source, install};
use crate::skills::link::{
    Desired, ORIGIN_ADOPTED, REAL_OPS, ReplaceOps, Replacement, SkillLedger, replace_folders_with,
    set_enabled,
};
use crate::skills::{LinkKind, SkillsEnv};
use crate::store::{Operation, Store};

static REAL: RealFs = RealFs;
static NO_REGISTRY: NoRegistry = NoRegistry;
static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: Duration::from_secs(60),
};

/// A temp root with `home/` and a data directory, removed on drop.
struct Fx {
    root: PathBuf,
    env: Env,
    home: LoadoutHome,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fx() -> Fx {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "loadout-replace-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("home")).unwrap();
    let os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    };
    let mut builder = Env::builder(os, root.join("home"))
        .clock(FixedClock(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600),
        ))
        .hostname("test-desk");
    if os == Os::Windows {
        builder = builder
            .known_folder(KnownFolder::LocalAppData, root.join("localappdata"))
            .known_folder(KnownFolder::RoamingAppData, root.join("appdata"));
    }
    let env = builder.build();
    let home = loadout_home(&env, &REAL, Some(&root.join("lh"))).unwrap();
    Fx { root, env, home }
}

impl Fx {
    fn sx(&self) -> SkillsEnv<'_> {
        SkillsEnv {
            env: &self.env,
            fs: &REAL,
            home: &self.home,
            registry: &NO_REGISTRY,
            runner: &RUNNER,
            link_kind: LinkKind::Junction,
        }
    }

    fn store(&self) -> Store<'_> {
        Store::open_or_init(&self.env, &REAL, &self.home, "test").unwrap()
    }

    fn claude_skills(&self) -> PathBuf {
        self.root.join("home").join(".claude").join("skills")
    }

    fn trash_entries(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.home.backups_dir().join("trash"))
            .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
            .unwrap_or_default()
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill(dir: &Path, name: &str) -> PathBuf {
    let folder = dir.join(name);
    write(
        &folder.join("SKILL.md"),
        &format!("---\nname: {name}\ndescription: Notes.\n---\n"),
    );
    write(&folder.join("docs").join("guide.md"), "# Guide\n");
    folder
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Skill `notes` in the store (from a copy elsewhere), enabled for Claude
/// Code, and Claude Code's own folder `notes` with the same content.
fn user_setup(fx: &Fx) -> (Store<'_>, PathBuf) {
    let store = fx.store();
    let sx = fx.sx();
    let source = skill(&fx.root.join("src"), "notes");
    install(&store, &sx, &InstallRequest::new(Source::Local(source))).unwrap();
    let folder = skill(&fx.claude_skills(), "notes");
    set_enabled(
        &store,
        &sx,
        "notes",
        &["claude-code".to_owned()],
        true,
        None,
    )
    .unwrap();
    (store, folder)
}

fn item(folder: &Path) -> Replacement {
    Replacement {
        client: "claude-code",
        folder: folder.to_path_buf(),
        entry: folder.to_path_buf(),
        skill_id: "notes".to_owned(),
    }
}

fn replace(
    fx: &Fx,
    store: &Store<'_>,
    folder: &Path,
    ops: &ReplaceOps,
) -> Result<PathBuf, CoreError> {
    replace_folders_with(store, &fx.sx(), &[item(folder)], ops)
        .unwrap()
        .remove(0)
}

/// A rename into the trash fails as it does across drives.
fn rename_across_drives(from: &Path, to: &Path) -> io::Result<()> {
    if to.components().any(|part| part.as_os_str() == "trash") {
        return Err(io::Error::from(io::ErrorKind::CrossesDevices));
    }
    std::fs::rename(from, to)
}

/// A copy that fails after writing part of the folder.
fn copy_half(_: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir(to)?;
    std::fs::write(to.join("SKILL.md"), "---\n")?;
    Err(io::Error::other("the disk is full"))
}

fn cannot_create(_: &dyn Fs, _: &Desired) -> Result<Option<String>, CoreError> {
    Err(CoreError::new(ErrorCode::IoError, "cannot create junction"))
}

fn aside(folder: &Path) -> PathBuf {
    folder.with_file_name(".notes.loadout-adopt")
}

#[test]
fn across_drives_the_folder_is_copied_to_the_trash_and_adopted() {
    let fx = fx();
    let (store, folder) = user_setup(&fx);
    let ops = ReplaceOps {
        rename: rename_across_drives,
        ..REAL_OPS
    };
    let trash = replace(&fx, &store, &folder, &ops).unwrap();
    assert!(is_link(&folder), "the folder is the managed link now");
    assert_eq!(
        std::fs::read_to_string(trash.join("docs").join("guide.md")).unwrap(),
        "# Guide\n",
        "the original is kept in the trash"
    );
    assert!(!is_link(&trash));
    assert!(!aside(&folder).exists(), "the folder set aside is removed");
    let ledger = SkillLedger::load(&REAL, &fx.home.state_dir()).unwrap();
    assert_eq!(ledger.get(&folder).unwrap().origin(), ORIGIN_ADOPTED);
}

#[test]
fn a_copy_that_fails_across_drives_puts_everything_back() {
    let fx = fx();
    let (store, folder) = user_setup(&fx);
    let ops = ReplaceOps {
        rename: rename_across_drives,
        copy_tree: copy_half,
        ..REAL_OPS
    };
    let err = replace(&fx, &store, &folder, &ops).unwrap_err();
    assert_eq!(err.code(), ErrorCode::IoError);
    assert!(!is_link(&folder), "the folder is back in place");
    assert_eq!(
        std::fs::read_to_string(folder.join("docs").join("guide.md")).unwrap(),
        "# Guide\n"
    );
    assert!(!aside(&folder).exists());
    assert!(fx.trash_entries().is_empty(), "the half copy is removed");
    let ledger = SkillLedger::load(&REAL, &fx.home.state_dir()).unwrap();
    assert!(ledger.get(&folder).is_none());
}

#[test]
fn a_folder_edited_after_the_commit_is_kept() {
    let fx = fx();
    let (store, folder) = user_setup(&fx);
    write(&folder.join("docs").join("guide.md"), "# Edited\n");
    let err = replace(&fx, &store, &folder, &REAL_OPS).unwrap_err();
    assert_eq!(err.code(), ErrorCode::EntryChanged);
    assert!(!is_link(&folder));
    assert_eq!(
        std::fs::read_to_string(folder.join("docs").join("guide.md")).unwrap(),
        "# Edited\n"
    );
    assert!(fx.trash_entries().is_empty());
}

#[test]
fn an_empty_folder_inside_is_kept() {
    let fx = fx();
    let (store, folder) = user_setup(&fx);
    std::fs::create_dir_all(folder.join("cache")).unwrap();
    let err = replace(&fx, &store, &folder, &REAL_OPS).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
    assert!(err.message().contains("empty folder cache"), "{err}");
    assert!(!is_link(&folder) && folder.join("cache").is_dir());
}

#[cfg(unix)]
#[test]
fn an_executable_file_is_kept() {
    use std::os::unix::fs::PermissionsExt;
    let fx = fx();
    let (store, folder) = user_setup(&fx);
    let script = folder.join("docs").join("guide.md");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = replace(&fx, &store, &folder, &REAL_OPS).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
    assert!(err.message().contains("executable"), "{err}");
    assert!(!is_link(&folder));
}

/// A trusted git project `web` enabling `notes` for Claude Code, with
/// Claude Code's project folder `notes`.
fn project_setup(fx: &Fx) -> (Store<'_>, PathBuf, PathBuf, git2::Repository) {
    let store = fx.store();
    let sx = fx.sx();
    let source = skill(&fx.root.join("src"), "notes");
    install(&store, &sx, &InstallRequest::new(Source::Local(source))).unwrap();
    let root = fx.root.join("projects").join("web");
    let repo = git2::Repository::init(&root).unwrap();
    write(
        &root.join(".loadout").join("project.local.toml"),
        "schema = 1\nid = \"web-0001\"\n\n[defaults]\nclients = [\"claude-code\"]\n\n\
         [skills]\nenable = [\"notes\"]\n",
    );
    store
        .transaction(Operation::new("test.edit", "test: know web"), |tx| {
            crate::layering::projects::set_known(tx, &REAL, "web-0001", &root)
        })
        .unwrap();
    let project = Project::load(&REAL, &root).unwrap().unwrap();
    let state = fx.home.state_dir();
    let mut trust = TrustStore::load(&REAL, &state).unwrap();
    trust.trust(&project);
    trust.save(&REAL, &state).unwrap();
    let folder = skill(&root.join(".claude").join("skills"), "notes");
    (store, root, folder, repo)
}

#[test]
fn an_entry_that_cannot_be_made_puts_the_folder_and_the_exclude_file_back() {
    let fx = fx();
    let (store, root, folder, _repo) = project_setup(&fx);
    let exclude = root.join(".git").join("info").join("exclude");
    let before = std::fs::read(&exclude).ok();
    let ops = ReplaceOps {
        create: cannot_create,
        ..REAL_OPS
    };
    let err = replace(&fx, &store, &folder, &ops).unwrap_err();
    assert_eq!(err.message(), "cannot create junction");
    assert!(!is_link(&folder) && folder.join("SKILL.md").is_file());
    assert!(!aside(&folder).exists());
    assert_eq!(
        std::fs::read(&exclude).ok(),
        before,
        "the real folder is not hidden from git"
    );
    assert!(fx.trash_entries().is_empty());

    // With the real operations the same folder is adopted and hidden.
    replace(&fx, &store, &folder, &REAL_OPS).unwrap();
    assert!(is_link(&folder));
    let text = std::fs::read_to_string(&exclude).unwrap();
    assert!(
        text.lines().any(|line| line == "/.claude/skills/notes"),
        "{text}"
    );
}

#[test]
fn a_folder_the_project_tracks_is_kept() {
    let fx = fx();
    let (store, root, folder, repo) = project_setup(&fx);
    let mut index = repo.index().unwrap();
    index
        .add_path(Path::new(".claude/skills/notes/SKILL.md"))
        .unwrap();
    index.write().unwrap();

    // Not offered by the scan.
    let scan = adopt::scan(Some(&store), &fx.sx(), Scope::Project, Some(&root), None).unwrap();
    assert!(scan.candidates.is_empty());
    assert_eq!(scan.problems.len(), 1);
    assert!(scan.problems[0].error.message().contains("tracks"));

    // Never replaced, should a replacement be asked for anyway.
    let err = replace(&fx, &store, &folder, &REAL_OPS).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
    assert!(err.message().contains("tracks"), "{err}");
    assert!(!is_link(&folder));
}
