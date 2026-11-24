//! Changes to a repository that make trust lapse (#24): its followed branch
//! was rewritten (force-push), or it was renamed or transferred (the host
//! redirects). Callers fetch with the system git and hand its output here.

use std::ffi::OsStr;
use std::time::{Duration, UNIX_EPOCH};

use crate::error::{CoreError, ErrorCode};
use crate::model::sources::LapseReason;
use crate::remote::git::RunOutput;
use crate::store::utc_datetime;

use super::SourceRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceChangeKind {
    /// The commit installed last is no longer in the followed branch.
    HistoryRewritten,
    /// The repository answers from another location.
    Moved,
}

impl SourceChangeKind {
    pub const fn as_str(self) -> &'static str {
        self.reason().as_str()
    }

    pub const fn reason(self) -> LapseReason {
        match self {
            SourceChangeKind::HistoryRewritten => LapseReason::HistoryRewritten,
            SourceChangeKind::Moved => LapseReason::Moved,
        }
    }
}

/// What changed about a source since it was last fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceChange {
    pub kind: SourceChangeKind,
    /// [`SourceChangeKind::Moved`]: the key of the new location.
    pub detail: Option<String>,
}

/// What git prints, with `LC_ALL=C` and without `--quiet`, when an HTTP
/// host redirects a repository (`remote-curl.c`).
const REDIRECT_WARNING: &str = "warning: redirecting to ";

/// The URL in git's `warning: redirecting to <url>` (an HTTP redirect of a
/// renamed or transferred repository), if `stderr` of a clone or fetch of
/// `url` has one. Only an `http(s)://` URL can redirect, and only a line
/// that starts with the warning counts: the remote's own messages
/// (`remote: ...`, or an SSH server's stderr) never do.
pub fn redirect_target(url: &str, stderr: &str) -> Option<String> {
    let scheme = url.split_once("://")?.0;
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    stderr.lines().find_map(|line| {
        let rest = line.strip_prefix(REDIRECT_WARNING)?;
        let url = rest.trim().trim_end_matches('/');
        let url = url.strip_suffix("/info/refs").unwrap_or(url);
        (!url.is_empty()).then(|| url.to_owned())
    })
}

/// The repository `redirect` (a [`redirect_target`]) names, when its key
/// differs from `source`'s: the repository moved.
pub fn target_of(source: &SourceRef, redirect: &str) -> Option<SourceRef> {
    SourceRef::from_git_url(redirect)
        .ok()
        .filter(|target| target.key() != source.key())
}

/// [`SourceChangeKind::Moved`] to `target` (a [`target_of`]).
pub fn moved_to(target: &SourceRef) -> SourceChange {
    SourceChange {
        kind: SourceChangeKind::Moved,
        detail: Some(target.key().to_owned()),
    }
}

/// [`SourceChangeKind::Moved`] when git's output of a clone or fetch of
/// `source` shows a redirect to another repository key than `source`'s.
pub fn moved(source: &SourceRef, output: &RunOutput) -> Option<SourceChange> {
    let redirect = redirect_target(source.url(), &output.stderr)?;
    target_of(source, &redirect).map(|target| moved_to(&target))
}

/// Git in one repository (a cache clone): runs `args`, returns the output.
pub type GitIn<'a> = dyn Fn(&[&OsStr]) -> Result<RunOutput, CoreError> + 'a;

/// Margin below the old commit's date when deepening a shallow clone, for
/// clock skew between the commits in between.
const SHALLOW_MARGIN: Duration = Duration::from_secs(86_400);

fn run_ok(git: &GitIn<'_>, args: &[&str]) -> Result<RunOutput, CoreError> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    git(&args)
}

/// Whether `new` (just fetched from `url`, following `git_ref` or the
/// default branch) no longer contains `old`, the commit installed last:
/// the branch was rewritten. A shallow clone is deepened first (to the old
/// commit's date, or completely when the old commit is not in it), so a
/// fast-forward is never mistaken for a rewrite; anything not proven a
/// fast-forward is a rewrite. A deepening fetch that fails while the
/// source answers (`git ls-remote`) is a rewrite too: the branch was
/// rolled back to a commit older than the deepening date. One that fails
/// because the source does not answer is `source_unreachable`.
pub fn history_rewritten(
    git: &GitIn<'_>,
    url: &str,
    git_ref: Option<&str>,
    old: &str,
    new: &str,
) -> Result<bool, CoreError> {
    if old.eq_ignore_ascii_case(new) {
        return Ok(false);
    }
    let hex =
        |text: &str| (4..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex(old) || !hex(new) {
        // Not a commit id git can be asked about (a hand-edited record).
        return Ok(true);
    }
    let shallow = run_ok(git, &["rev-parse", "--is-shallow-repository"])?;
    if shallow.success && shallow.stdout.trim() == "true" {
        let date = run_ok(
            git,
            &["show", "-s", "--format=%ct", &format!("{old}^{{commit}}")],
        )?;
        let seconds = date
            .success
            .then(|| date.stdout.trim().parse::<u64>().ok())
            .flatten();
        let deepen = match seconds {
            Some(seconds) => {
                let since = (UNIX_EPOCH + Duration::from_secs(seconds))
                    .checked_sub(SHALLOW_MARGIN)
                    .unwrap_or(UNIX_EPOCH);
                format!("--shallow-since={}", utc_datetime(since))
            }
            None => "--unshallow".to_owned(),
        };
        let mut args = vec!["fetch", "--quiet", &deepen, "--", url];
        if let Some(git_ref) = git_ref {
            args.push(git_ref);
        }
        let output = run_ok(git, &args)?;
        if !output.success {
            let answers = run_ok(git, &["ls-remote", "--quiet", "--", url])?;
            if answers.success {
                return Ok(true);
            }
            let shown = SourceRef::from_git_url(url).map_or_else(
                |_| "of this skill".to_owned(),
                |source| source.url().to_owned(),
            );
            return Err(CoreError::new(
                ErrorCode::SourceUnreachable,
                SourceRef::redact_userinfo(&format!(
                    "the source {shown} cannot be reached to check its history: {}",
                    output
                        .stderr
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("git failed")
                        .trim()
                )),
            ));
        }
    }
    let ancestor = run_ok(git, &["merge-base", "--is-ancestor", old, new])?;
    Ok(!ancestor.success)
}
