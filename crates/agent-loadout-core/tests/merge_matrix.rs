//! The pull merge (SPEC 9.9) as a matrix: entity type x change kind x
//! side. One side's change is taken, identical changes are taken, and
//! different changes to the same field (or a delete against a change)
//! conflict: never a silent local win. Machine files follow their single
//! writer instead.

use std::collections::BTreeMap;

use agent_loadout_core::store::merge::{Files, MergePolicy, Side, merge_trees};

const ME: &str = "01JCX3R8Q6T5W1V9Y2Z7A4B6C8";
const OTHER: &str = "01JCX5B7K3M9N1P5Q7R3S9T1V5";

/// An entity type: its file, a base text and two edits of one field, and
/// an edit of another field of the same file.
struct Entity {
    file: String,
    base: &'static str,
    /// The field edited in `a` and `b` (for conflicts).
    a: &'static str,
    b: &'static str,
    /// An edit of a different field.
    other: &'static str,
    /// The merge of `a` and `other`, field by field.
    a_and_other: &'static str,
}

fn entities() -> Vec<Entity> {
    vec![
        Entity {
            file: "servers/github.toml".into(),
            base: "schema = 1\nid = \"github\"\ntransport = \"stdio\"\ncommand = \"npx\"\n\n[env]\nTOOLSETS = \"repos\"\n",
            a: "schema = 1\nid = \"github\"\ntransport = \"stdio\"\ncommand = \"npx\"\n\n[env]\nTOOLSETS = \"repos,issues\"\n",
            b: "schema = 1\nid = \"github\"\ntransport = \"stdio\"\ncommand = \"npx\"\n\n[env]\nTOOLSETS = \"actions\"\n",
            other: "schema = 1\nid = \"github\"\ntransport = \"stdio\"\ncommand = \"uvx\"\n\n[env]\nTOOLSETS = \"repos\"\n",
            a_and_other: "schema = 1\nid = \"github\"\ntransport = \"stdio\"\ncommand = \"uvx\"\n\n[env]\nTOOLSETS = \"repos,issues\"\n",
        },
        Entity {
            file: "secrets.toml".into(),
            base: "schema = 1\n\n[secrets.PAT]\nref = \"env:PAT\"\n\n[secrets.KEY]\nref = \"env:KEY\"\n",
            a: "schema = 1\n\n[secrets.PAT]\nref = \"keyring://loadout/PAT\"\n\n[secrets.KEY]\nref = \"env:KEY\"\n",
            b: "schema = 1\n\n[secrets.PAT]\nref = \"op://Vault/Item/field\"\n\n[secrets.KEY]\nref = \"env:KEY\"\n",
            other: "schema = 1\n\n[secrets.PAT]\nref = \"env:PAT\"\n\n[secrets.KEY]\nref = \"env:OTHER_KEY\"\n",
            a_and_other: "schema = 1\n\n[secrets.PAT]\nref = \"keyring://loadout/PAT\"\n\n[secrets.KEY]\nref = \"env:OTHER_KEY\"\n",
        },
        Entity {
            file: "clients.toml".into(),
            base: "schema = 1\n\n[omp]\nprofiles = [\"default\"]\n\n[pi]\nnative_disable = false\n",
            a: "schema = 1\n\n[omp]\nprofiles = [\"work\"]\n\n[pi]\nnative_disable = false\n",
            b: "schema = 1\n\n[omp]\nprofiles = [\"home\"]\n\n[pi]\nnative_disable = false\n",
            other: "schema = 1\n\n[omp]\nprofiles = [\"default\"]\n\n[pi]\nnative_disable = true\n",
            a_and_other: "schema = 1\n\n[omp]\nprofiles = [\"work\"]\n\n[pi]\nnative_disable = true\n",
        },
        Entity {
            file: "settings.toml".into(),
            base: "schema = 1\ntheme = \"light\"\napply_mode = \"instant\"\n",
            a: "schema = 1\ntheme = \"dark\"\napply_mode = \"instant\"\n",
            b: "schema = 1\ntheme = \"system\"\napply_mode = \"instant\"\n",
            other: "schema = 1\ntheme = \"light\"\napply_mode = \"review\"\n",
            a_and_other: "schema = 1\ntheme = \"dark\"\napply_mode = \"review\"\n",
        },
        Entity {
            file: "projects/app-7k2f/project.toml".into(),
            base: "schema = 1\nid = \"app-7k2f\"\nname = \"App\"\n\n[servers.notes]\nenabled = false\n",
            a: "schema = 1\nid = \"app-7k2f\"\nname = \"App One\"\n\n[servers.notes]\nenabled = false\n",
            b: "schema = 1\nid = \"app-7k2f\"\nname = \"App Two\"\n\n[servers.notes]\nenabled = false\n",
            other: "schema = 1\nid = \"app-7k2f\"\nname = \"App\"\n\n[servers.notes]\nenabled = true\n",
            a_and_other: "schema = 1\nid = \"app-7k2f\"\nname = \"App One\"\n\n[servers.notes]\nenabled = true\n",
        },
        Entity {
            file: "skill-meta/review.toml".into(),
            base: "schema = 1\nid = \"review\"\nversion = \"1\"\nsource = \"local\"\n",
            a: "schema = 1\nid = \"review\"\nversion = \"2\"\nsource = \"local\"\n",
            b: "schema = 1\nid = \"review\"\nversion = \"3\"\nsource = \"local\"\n",
            other: "schema = 1\nid = \"review\"\nversion = \"1\"\nsource = \"git\"\n",
            a_and_other: "schema = 1\nid = \"review\"\nversion = \"2\"\nsource = \"git\"\n",
        },
        Entity {
            file: "skills/review/SKILL.md".into(),
            base: "# Review\n\nStep one.\n\nStep two.\n\nStep three.\n",
            a: "# Review\n\nStep one, carefully.\n\nStep two.\n\nStep three.\n",
            b: "# Review\n\nStep one, quickly.\n\nStep two.\n\nStep three.\n",
            other: "# Review\n\nStep one.\n\nStep two.\n\nStep three, done.\n",
            a_and_other: "# Review\n\nStep one, carefully.\n\nStep two.\n\nStep three, done.\n",
        },
    ]
}

fn files(pairs: &[(&str, &str)]) -> Files {
    pairs
        .iter()
        .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
        .collect()
}

fn theirs_newer(_: &str) -> Side {
    Side::Theirs
}

fn merge(base: &Files, ours: &Files, theirs: &Files) -> agent_loadout_core::store::merge::Merged {
    merge_trees(
        base,
        ours,
        theirs,
        &MergePolicy {
            machine_id: ME,
            newer: &theirs_newer,
        },
    )
}

fn text(merged: &Files, file: &str) -> Option<String> {
    merged
        .get(file)
        .map(|bytes| String::from_utf8(bytes.clone()).unwrap())
}

#[test]
fn one_sided_and_identical_changes_are_taken() {
    for e in entities() {
        let f = e.file.as_str();
        let base = files(&[(f, e.base)]);
        let changed = files(&[(f, e.a)]);
        let empty = Files::new();
        // (kind, ours, theirs, expected)
        let cases: [(&str, &Files, &Files, Option<&str>); 6] = [
            ("modify ours", &changed, &base, Some(e.a)),
            ("modify theirs", &base, &changed, Some(e.a)),
            ("modify both same", &changed, &changed, Some(e.a)),
            ("delete ours", &empty, &base, None),
            ("delete theirs", &base, &empty, None),
            ("delete both", &empty, &empty, None),
        ];
        for (kind, ours, theirs, expected) in cases {
            let merged = merge(&base, ours, theirs);
            assert!(
                merged.conflicts.is_empty(),
                "{f} {kind}: {:?}",
                merged.conflicts
            );
            assert_eq!(text(&merged.files, f).as_deref(), expected, "{f} {kind}");
        }
        // Added on one side, or the same on both.
        for (kind, ours, theirs) in [
            ("add ours", &changed, &empty),
            ("add theirs", &empty, &changed),
            ("add both same", &changed, &changed),
        ] {
            let merged = merge(&Files::new(), ours, theirs);
            assert!(merged.conflicts.is_empty(), "{f} {kind}");
            assert_eq!(text(&merged.files, f).as_deref(), Some(e.a), "{f} {kind}");
        }
    }
}

#[test]
fn different_fields_merge_and_the_same_field_conflicts() {
    for e in entities() {
        let f = e.file.as_str();
        let base = files(&[(f, e.base)]);
        // Different fields (lines for text): both kept, either way round.
        for (ours, theirs) in [(e.a, e.other), (e.other, e.a)] {
            let merged = merge(&base, &files(&[(f, ours)]), &files(&[(f, theirs)]));
            assert!(merged.conflicts.is_empty(), "{f}: {:?}", merged.conflicts);
            assert_eq!(
                text(&merged.files, f).as_deref(),
                Some(e.a_and_other),
                "{f}"
            );
        }
        // The same field changed differently: a conflict, nothing merged,
        // and never ours silently.
        let merged = merge(&base, &files(&[(f, e.a)]), &files(&[(f, e.b)]));
        assert!(!merged.conflicts.is_empty(), "{f}");
        let conflict = &merged.conflicts[0];
        assert_eq!(conflict.file, f);
        assert_ne!(conflict.ours, conflict.theirs);
        if f.ends_with(".toml") {
            assert!(conflict.field.is_some(), "{f}: field-level conflict");
        } else {
            assert!(conflict.field.is_none(), "{f}: whole-file conflict");
        }
        assert!(
            !merged.files.contains_key(f),
            "{f}: conflicting file not merged"
        );
        // Added on both sides differently.
        let merged = merge(&Files::new(), &files(&[(f, e.a)]), &files(&[(f, e.b)]));
        assert!(!merged.conflicts.is_empty(), "{f} add/add");
        // Deleted on one side, changed on the other, either way round.
        for (ours, theirs) in [
            (Files::new(), files(&[(f, e.a)])),
            (files(&[(f, e.a)]), Files::new()),
        ] {
            let merged = merge(&base, &ours, &theirs);
            assert_eq!(merged.conflicts.len(), 1, "{f} delete/modify");
            assert_eq!(merged.conflicts[0].field, None);
        }
    }
}

#[test]
fn toml_merges_keep_ours_layout_and_comments() {
    let base = "# Comment kept\nschema = 1\nid = \"github\" # trailing\ntransport = \"stdio\"\ncommand = \"npx\"\n";
    let ours = "# Comment kept\nschema = 1\nid = \"github\" # trailing\ntransport = \"stdio\"\ncommand = \"npx\"\ndescription = \"Mine\"\n";
    let theirs = "schema = 1\nid = \"github\"\ntransport = \"stdio\"\ncommand = \"uvx\"\n";
    let f = "servers/github.toml";
    let merged = merge(
        &files(&[(f, base)]),
        &files(&[(f, ours)]),
        &files(&[(f, theirs)]),
    );
    assert!(merged.conflicts.is_empty());
    assert_eq!(
        text(&merged.files, f).unwrap(),
        "# Comment kept\nschema = 1\nid = \"github\" # trailing\ntransport = \"stdio\"\ncommand = \"uvx\"\ndescription = \"Mine\"\n"
    );
    // A key removed on their side only.
    let theirs = "# Comment kept\nschema = 1\nid = \"github\" # trailing\ncommand = \"npx\"\n";
    let merged = merge(
        &files(&[(f, base)]),
        &files(&[(f, ours)]),
        &files(&[(f, theirs)]),
    );
    assert!(merged.conflicts.is_empty());
    assert!(!text(&merged.files, f).unwrap().contains("transport"));
}

#[test]
fn machine_files_follow_their_single_writer() {
    let mine = format!("machines/{ME}.toml");
    let theirs_file = format!("machines/{OTHER}.toml");
    let applied = format!("machines/{OTHER}.applied.toml");
    let overlay = |label: &str| {
        format!("schema = 1\nmachine_id = \"x\"\nlabel = \"{label}\"\nos = \"linux\"\n")
    };
    let base = files(&[
        (&mine, &overlay("base")),
        (&theirs_file, &overlay("base")),
        (&applied, "schema = 1\n"),
    ]);
    let ours = files(&[
        (&mine, &overlay("mine-ours")),
        (&theirs_file, &overlay("other-ours")),
        (&applied, "schema = 1\n# ours\n"),
    ]);
    let theirs = files(&[
        (&mine, &overlay("mine-theirs")),
        (&theirs_file, &overlay("other-theirs")),
        (&applied, "schema = 1\n# theirs\n"),
    ]);
    // Both sides changed every file: no conflict. This machine keeps its
    // own; the other machine's come from the side that touched them last.
    let merged = merge(&base, &ours, &theirs);
    assert!(merged.conflicts.is_empty(), "{:?}", merged.conflicts);
    assert_eq!(text(&merged.files, &mine).unwrap(), overlay("mine-ours"));
    assert_eq!(
        text(&merged.files, &theirs_file).unwrap(),
        overlay("other-theirs")
    );
    assert_eq!(
        text(&merged.files, &applied).unwrap(),
        "schema = 1\n# theirs\n"
    );
    let ours_newer = |_: &str| Side::Ours;
    let merged = merge_trees(
        &base,
        &ours,
        &theirs,
        &MergePolicy {
            machine_id: ME,
            newer: &ours_newer,
        },
    );
    assert_eq!(
        text(&merged.files, &theirs_file).unwrap(),
        overlay("other-ours")
    );
    // Deleted on one side, changed on the other: still the writer's side.
    let merged = merge(&base, &BTreeMap::new(), &theirs);
    assert!(merged.conflicts.is_empty());
    assert_eq!(
        text(&merged.files, &theirs_file).unwrap(),
        overlay("other-theirs")
    );
    assert_eq!(text(&merged.files, &mine), None);
}

#[test]
fn binary_changes_on_both_sides_conflict() {
    let f = "skills/review/logo.png";
    let base: Files = [(f.to_owned(), vec![0x89, b'P', b'N', b'G', 0, 1])].into();
    let ours: Files = [(f.to_owned(), vec![0x89, b'P', b'N', b'G', 0, 2])].into();
    let theirs: Files = [(f.to_owned(), vec![0x89, b'P', b'N', b'G', 0, 3])].into();
    let merged = merge(&base, &ours, &theirs);
    assert_eq!(merged.conflicts.len(), 1);
    let merged = merge(&base, &ours, &base);
    assert_eq!(merged.files[f], ours[f]);
}
