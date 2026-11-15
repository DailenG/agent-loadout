//! Rust twin of `scripts/check-no-baked-ids.sh`, so the check also runs in
//! `cargo test` on Windows. Scans tracked and untracked, non-ignored files.

use std::path::Path;
use std::process::Command;

/// Design documents that name SkillDock's baked-in id and dead upstream on
/// purpose, to explain what this project avoids.
const ALLOWED_FILES: [&str; 2] = ["SPEC.md", "LANDSCAPE.md"];

#[test]
fn repository_has_no_baked_ids() {
    // Assembled from fragments so this file does not match itself.
    let needles = [
        ["Ov2", "3li"].concat(),
        ["Iv", "1."].concat(),
        ["wang", "huan9"].concat(),
    ];
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let listing = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(&repo)
        .output();
    let files = match listing {
        Ok(output) if output.status.success() => output.stdout,
        _ => {
            eprintln!("notice: git unavailable, skipping the baked-id check");
            return;
        }
    };

    let mut hits = Vec::new();
    for name in files
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = String::from_utf8_lossy(name);
        if ALLOWED_FILES.contains(&name.as_ref()) {
            continue;
        }
        // Deleted but still tracked files have nothing to scan.
        let Ok(bytes) = std::fs::read(repo.join(name.as_ref())) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for (index, line) in text.lines().enumerate() {
            for needle in &needles {
                if line.contains(needle.as_str()) {
                    hits.push(format!("{name}:{}: {needle}", index + 1));
                }
            }
        }
    }
    assert!(hits.is_empty(), "baked-in ids found:\n{}", hits.join("\n"));
}
