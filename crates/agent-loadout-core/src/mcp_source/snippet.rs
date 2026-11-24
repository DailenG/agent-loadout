//! Client config text in any client's shape (SPEC 13.2 paste-anything,
//! and the committed configs of a repository, #5): which client's mapper
//! reads it and which entries it holds.

use crate::adapters::{ADAPTERS, AdapterSpec, adapter};
use crate::env::Os;
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, Format, NativeMap, NativeValue};

/// The entries a snippet holds and the client whose shape it has.
#[derive(Clone)]
pub struct Entries {
    pub spec: &'static AdapterSpec,
    /// Entry name and native value, in document order.
    pub entries: Vec<(String, NativeValue)>,
}

/// `text` read as JSONC, TOML or YAML, whichever parses first.
pub fn parse_any(text: &str) -> Option<Document> {
    [Format::Jsonc, Format::Toml, Format::Yaml]
        .into_iter()
        .find_map(|format| Document::parse(text.as_bytes(), format).ok())
}

fn same_family(a: Format, b: Format) -> bool {
    let family = |format: Format| match format {
        Format::Toml => 1,
        Format::Yaml => 2,
        _ => 0,
    };
    family(a) == family(b)
}

/// The entries of `doc`: the container of the first client (`hint` first,
/// then table order) whose container holds entries; else a bare map of
/// entries, or a single entry (named `server`), read with Claude Code's
/// mapper (Codex's for TOML). `unexpected_shape` when it holds none.
pub fn entries(
    doc: &Document,
    os: Os,
    hint: Option<&'static AdapterSpec>,
) -> Result<Entries, CoreError> {
    for spec in hint.into_iter().chain(ADAPTERS.iter().copied()) {
        let Some(mcp) = spec.mcp.as_ref() else {
            continue;
        };
        if !same_family(mcp.format, doc.format()) {
            continue;
        }
        let Ok(keys) = mcp.container.keys(os, None) else {
            continue;
        };
        if keys.is_empty() {
            continue;
        }
        let container: Vec<&str> = keys.iter().map(String::as_str).collect();
        if let Ok(names) = doc.list(&container)
            && !names.is_empty()
        {
            let entries = names
                .into_iter()
                .filter_map(|name| match doc.get(&container, &name) {
                    Ok(Some(value)) => Some((name, value)),
                    _ => None,
                })
                .collect();
            return Ok(Entries { spec, entries });
        }
    }
    let fallback = match doc.format() {
        Format::Toml => "codex",
        _ => "claude-code",
    };
    let spec = hint.unwrap_or_else(|| adapter(fallback).expect("adapter in the table"));
    let names = doc.list(&[]).unwrap_or_default();
    if names.iter().any(|name| name == "command" || name == "url") {
        let mut root = NativeMap::new();
        for name in &names {
            if let Ok(Some(value)) = doc.get(&[], name) {
                root.insert(name.clone(), value);
            }
        }
        return Ok(Entries {
            spec,
            entries: vec![("server".to_owned(), root.into())],
        });
    }
    let is_entry = |value: &NativeValue| {
        value
            .as_object()
            .is_some_and(|map| map.contains_key("command") || map.contains_key("url"))
    };
    let entries: Vec<(String, NativeValue)> = names
        .into_iter()
        .filter_map(|name| match doc.get(&[], &name) {
            Ok(Some(value)) if is_entry(&value) => Some((name, value)),
            _ => None,
        })
        .collect();
    if entries.is_empty() {
        return Err(CoreError::new(
            ErrorCode::UnexpectedShape,
            "the text holds no MCP server entry",
        ));
    }
    Ok(Entries { spec, entries })
}
