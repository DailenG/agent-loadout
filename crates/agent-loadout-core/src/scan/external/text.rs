//! Text helpers of the external scanners' adapters: the JSON document in
//! a scanner's stdout, paths made relative to the scanned folder, and
//! one-line messages with invisible characters spelled out.

use std::path::Path;

use serde_json::Value;

/// The JSON document, also when something was printed around it.
pub(super) fn json_of(text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (start < end)
        .then(|| serde_json::from_str(&text[start..=end]).ok())
        .flatten()
}

/// `path` relative to `root`, `/`-separated; `None` for a path outside it
/// (never an absolute path in a finding).
pub(super) fn relative(path: &str, root: &Path) -> Option<String> {
    let slashed = path.replace('\\', "/");
    let root_text = root.to_string_lossy().replace('\\', "/");
    let root_text = root_text.trim_end_matches('/');
    let rest = if let Some(rest) = strip_prefix_ci(&slashed, root_text) {
        rest.trim_start_matches('/').to_owned()
    } else {
        slashed
    };
    let rest = rest.strip_prefix("./").map(str::to_owned).unwrap_or(rest);
    let absolute =
        rest.starts_with('/') || rest.starts_with('~') || rest.as_bytes().get(1) == Some(&b':');
    let escapes = rest.split('/').any(|part| part == "..");
    (!rest.is_empty() && !absolute && !escapes).then_some(rest)
}

/// `text` without `prefix`, ASCII case-insensitively (Windows paths).
fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() || text.len() < prefix.len() || !text.is_char_boundary(prefix.len()) {
        return None;
    }
    let (head, rest) = text.split_at(prefix.len());
    (head.eq_ignore_ascii_case(prefix) && (rest.is_empty() || rest.starts_with('/')))
        .then_some(rest)
}

/// `text` with the scanned folder's path replaced by `<skill>`: its native
/// form, the `/` form, and on Windows the backslash-doubled form Python's
/// error reprs and JSON-escaped text print (`C:\\Users\\...\\skill`).
pub(super) fn strip_root(text: &str, root: &Path) -> String {
    let native = root.to_string_lossy().into_owned();
    if native.is_empty() {
        return text.to_owned();
    }
    let mut out = text.to_owned();
    if native.contains('\\') {
        out = out.replace(&native.replace('\\', "\\\\"), "<skill>");
    }
    let forward = native.replace('\\', "/");
    out.replace(&native, "<skill>").replace(&forward, "<skill>")
}

/// One line: control characters as spaces, invisible characters (bidi
/// controls, zero-width, tags) spelled out as `\u{..}`, runs of spaces
/// collapsed, at most `max` characters (with an ellipsis).
pub(super) fn clean(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max + 3));
    let mut space = false;
    for ch in text.chars() {
        if matches!(
            ch,
            '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{E0000}'..='\u{E01EF}'
        ) {
            out.push_str(&format!("\\u{{{:X}}}", u32::from(ch)));
            space = false;
            continue;
        }
        let ch = if ch.is_control() { ' ' } else { ch };
        if ch == ' ' {
            if space || out.is_empty() {
                continue;
            }
            space = true;
        } else {
            space = false;
        }
        out.push(ch);
    }
    let out = out.trim_end().to_owned();
    if out.chars().count() <= max {
        return out;
    }
    let mut cut: String = out.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}
