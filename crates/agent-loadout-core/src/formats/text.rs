//! Byte-level style of a text file: UTF-8 BOM, line endings, final newline.
//!
//! Editors work on BOM-free text with `\n` line endings; [`decode`] and
//! [`encode`] translate exactly, so untouched lines come back byte for byte.

use crate::error::{CoreError, ErrorCode};

const BOM: char = '\u{feff}';

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineEnding {
    Lf,
    Crlf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextStyle {
    /// The file starts with a UTF-8 byte order mark.
    pub bom: bool,
    /// `Crlf` when every line break is `\r\n`. A file that mixes both is
    /// `Lf`: its breaks are kept as they are and new lines get `\n`.
    pub line_ending: LineEnding,
    /// The file ends with a line break (an empty file counts as one, so a new
    /// file gets one).
    pub final_newline: bool,
}

/// UTF-8 text without BOM and, for CRLF files, with `\n` line endings.
pub fn decode(bytes: &[u8]) -> Result<(String, TextStyle), CoreError> {
    let text = std::str::from_utf8(bytes).map_err(|err| {
        CoreError::new(ErrorCode::ParseError, format!("file is not UTF-8: {err}"))
    })?;
    let bom = text.starts_with(BOM);
    let text = text.strip_prefix(BOM).unwrap_or(text);
    let crlf = text.contains('\n') && text.matches('\n').count() == text.matches("\r\n").count();
    let style = TextStyle {
        bom,
        line_ending: if crlf {
            LineEnding::Crlf
        } else {
            LineEnding::Lf
        },
        final_newline: text.is_empty() || text.ends_with('\n'),
    };
    let text = if crlf {
        text.replace("\r\n", "\n")
    } else {
        text.to_owned()
    };
    Ok((text, style))
}

/// The inverse of [`decode`]. Also restores the presence or absence of a
/// final line break, which an edit at the end of the file may have changed.
pub fn encode(text: &str, style: TextStyle) -> Vec<u8> {
    let mut text = text.to_owned();
    if style.final_newline {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
    } else if text.ends_with('\n') {
        text.pop();
    }
    if style.line_ending == LineEnding::Crlf {
        text = text.replace('\n', "\r\n");
    }
    let mut bytes = Vec::with_capacity(text.len() + 3);
    if style.bom {
        bytes.extend_from_slice(BOM.encode_utf8(&mut [0; 4]).as_bytes());
    }
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// 1-based line and column of byte `offset` in `text`, for messages.
pub fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(before, |newline| &before[newline + 1..])
        .chars()
        .count()
        + 1;
    (line, column)
}
