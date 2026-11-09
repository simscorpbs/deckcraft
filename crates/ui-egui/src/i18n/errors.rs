//! Engine and file errors, worded for the interface at the moment they are drawn (status bar,
//! toast, Backstage). The engine's text is untouched: the control channel, the CLI and MCP keep
//! the English message, and `app.status` keeps it too. Only the common messages are mapped; any
//! other message is shown as the engine wrote it.

use std::borrow::Cow;

use super::{current, fmt, t};

/// Messages with a runtime part, in the engine's wording. Each is also a catalog row, with the same
/// `{placeholders}`. A placeholder takes the text up to the next literal piece of the template.
pub(super) const TEMPLATES: &[&str] = &[
    "{name}: not a presentation DeckCraft can read",
    "{name} is password-protected. DeckCraft can't open password-protected presentations yet.",
    "{name}: DeckCraft can't open this kind of file",
    "Couldn't save {name}: {reason}",
    "{path}: not a regular file",
    "{path}: {reason} (another program has the file open; close it and save again)",
    "{name}: the picture can't be read",
    "{name} was not applied: the presentation it was chosen for is closed",
    "{name} was not applied: {reason}",
    "not a PPTX file: {detail}",
    "unsupported: {detail}",
    "write failed: {detail}",
    "malformed XML: {detail}",
    "some version history could not be deleted: {reason}",
    "{path} can't be reopened as it is now; use Keep both instead",
];

/// The operating system's own wording of the common file errors (Rust's `io::Error` text), which
/// is English on most systems. A system in Portuguese already words them, so they pass through.
pub(super) const OS_REASONS: &[&str] = &[
    "No such file or directory (os error 2)",
    "The system cannot find the file specified. (os error 2)",
    "The system cannot find the path specified. (os error 3)",
    "Permission denied (os error 13)",
    "Access is denied. (os error 5)",
    "Is a directory (os error 21)",
    "Read-only file system (os error 30)",
    "No space left on device (os error 28)",
    "There is not enough space on the disk. (os error 112)",
    "File exists (os error 17)",
    "The process cannot access the file because it is being used by another process. (os error 32)",
];

/// `msg`, an engine or file error, in the interface language. English, and any message that is
/// not mapped, come back unchanged.
pub fn user_error(msg: &str) -> Cow<'_, str> {
    if current().code() == "en" || msg.is_empty() {
        return Cow::Borrowed(msg);
    }
    let direct = t(msg);
    if direct != msg {
        return Cow::Borrowed(direct);
    }
    for template in TEMPLATES {
        if let Some(values) = capture(template, msg) {
            let values: Vec<(String, String)> = values.into_iter().map(|(k, v)| (k, reason(&v).into_owned())).collect();
            let refs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            return Cow::Owned(fmt(t(template), &refs));
        }
    }
    // `{path}: {operating system's reason}`, the shape of every failed read or write.
    for (at, _) in msg.match_indices(": ") {
        let (path, rest) = (&msg[..at], &msg[at + 2..]);
        if OS_REASONS.contains(&rest) {
            return Cow::Owned(format!("{path}: {}", t(rest)));
        }
    }
    Cow::Borrowed(msg)
}

/// A captured value in the interface language when it is a known file-system error.
fn reason(value: &str) -> Cow<'_, str> {
    if OS_REASONS.contains(&value) { Cow::Borrowed(t(value)) } else { Cow::Borrowed(value) }
}

/// The `{name}` values of `msg` when it is `template` filled in, else `None`.
fn capture(template: &str, msg: &str) -> Option<Vec<(String, String)>> {
    // Split into alternating literal and placeholder pieces.
    let mut pieces: Vec<(bool, &str)> = vec![];
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let close = open + rest[open..].find('}')?;
        if open > 0 {
            pieces.push((false, &rest[..open]));
        }
        pieces.push((true, &rest[open + 1..close]));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        pieces.push((false, rest));
    }
    let mut out = vec![];
    let mut at = 0;
    let mut i = 0;
    while i < pieces.len() {
        let (is_name, text) = pieces[i];
        if !is_name {
            if !msg[at..].starts_with(text) {
                return None;
            }
            at += text.len();
        } else {
            let end = match pieces.get(i + 1) {
                None => msg.len(),
                Some((false, literal)) => {
                    // The last literal must close the message, so look for it from the end.
                    if i + 2 == pieces.len() { msg[at..].rfind(literal).map(|p| at + p)? } else { at + msg[at..].find(literal)? }
                }
                Some((true, _)) => return None,
            };
            if end < at {
                return None;
            }
            out.push((text.to_string(), msg[at..end].to_string()));
            at = end;
        }
        i += 1;
    }
    (at == msg.len()).then_some(out)
}
