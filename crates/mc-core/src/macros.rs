//! Local text macros (CHT-05) and the station information they insert
//! (CHT-06). A macro is expanded into text for the operator's draft; nothing
//! here sends anything, and the operator edits the text before sending.

use crate::app::MAX_TEXT_BYTES;
use crate::profile::FieldError;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Most characters in a macro's name.
pub const NAME_MAX: usize = 32;
/// Most macros one profile keeps.
pub const MAX_MACROS: usize = 50;
/// The longest a tag name is (`<ABOUT>`, `<MYCALL>`): longer is not a tag.
const TAG_MAX: usize = 6;

/// A macro as the UI sees it. An empty `id` on save means a new macro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MacroDef {
    pub id: String,
    pub name: String,
    pub body: String,
    /// Listed under "Quick messages" at the top of the composer's menu (M3c B8).
    #[serde(default)]
    pub quick: bool,
}

/// Everything a tag can expand to. A `None` (or empty) value is missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MacroContext {
    /// The peer being written to, if a conversation is open.
    pub call: Option<String>,
    pub mycall: String,
    pub name: Option<String>,
    pub qth: Option<String>,
    /// The profile's Maidenhead locator.
    pub loc: Option<String>,
    pub rig: Option<String>,
    pub pwr: Option<String>,
    pub ant: Option<String>,
    pub about: Option<String>,
    pub email: Option<String>,
    /// `HH:MM`, UTC.
    pub utc: String,
    /// The operator's explicit "insert email" action: without it `<EMAIL>`
    /// is left as it is.
    pub allow_email: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// Tags that had no value and were left in the text, once each, in
    /// order of first appearance.
    pub missing: Vec<String>,
}

fn present(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

/// Replace known tags in `body`. A tag with no value stays literally and is
/// reported; an unknown tag, a lower-case one or an unclosed `<` is plain
/// text. Values are not scanned again. A result over `MAX_TEXT_BYTES` is an
/// error, never a silent cut.
pub fn expand(body: &str, ctx: &MacroContext) -> Result<Expanded, Vec<FieldError>> {
    let mut text = String::with_capacity(body.len());
    let mut missing: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let tag = after.find('>').filter(|&end| end <= TAG_MAX).map(|end| &after[..end]);
        let value = tag.and_then(|t| match t {
            "CALL" => Some(present(&ctx.call)),
            "MYCALL" => Some(Some(ctx.mycall.as_str()).filter(|s| !s.is_empty())),
            "NAME" => Some(present(&ctx.name)),
            "QTH" => Some(present(&ctx.qth)),
            "LOC" => Some(present(&ctx.loc)),
            "RIG" => Some(present(&ctx.rig)),
            "PWR" => Some(present(&ctx.pwr)),
            "ANT" => Some(present(&ctx.ant)),
            "ABOUT" => Some(present(&ctx.about)),
            "UTC" => Some(Some(ctx.utc.as_str()).filter(|s| !s.is_empty())),
            "EMAIL" if ctx.allow_email => Some(present(&ctx.email)),
            _ => None,
        });
        match (tag, value) {
            (Some(t), Some(Some(v))) => {
                text.push_str(v);
                rest = &after[t.len() + 1..];
            }
            (Some(t), Some(None)) => {
                text.push('<');
                text.push_str(t);
                text.push('>');
                if !missing.iter().any(|m| m == t) {
                    missing.push(t.to_string());
                }
                rest = &after[t.len() + 1..];
            }
            // Not a tag we know: this '<' is just text.
            _ => {
                text.push('<');
                rest = after;
            }
        }
        if text.len() > MAX_TEXT_BYTES {
            break;
        }
    }
    text.push_str(rest);
    if text.len() > MAX_TEXT_BYTES {
        return Err(vec![FieldError {
            field: "text".into(),
            message: format!("The expanded text would be over {MAX_TEXT_BYTES} bytes; shorten the macro"),
        }]);
    }
    Ok(Expanded { text, missing })
}

fn bad(field: &str, message: impl Into<String>) -> Vec<FieldError> {
    vec![FieldError { field: field.into(), message: message.into() }]
}

/// The normalised macro (name trimmed), or why it is refused. A body may hold
/// line breaks and tabs but no other control characters.
pub fn validate(def: &MacroDef) -> Result<MacroDef, Vec<FieldError>> {
    let mut errors = Vec::new();
    let name = def.name.trim();
    if name.is_empty() {
        errors.extend(bad("name", "Give the macro a name"));
    } else if name.chars().count() > NAME_MAX {
        errors.extend(bad("name", format!("At most {NAME_MAX} characters")));
    } else if name.chars().any(char::is_control) {
        errors.extend(bad("name", "No control characters"));
    }
    if def.body.trim().is_empty() {
        errors.extend(bad("body", "The macro text is empty"));
    } else if def.body.len() > MAX_TEXT_BYTES {
        errors.extend(bad("body", format!("At most {MAX_TEXT_BYTES} bytes; this one is {} bytes", def.body.len())));
    } else if def.body.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        errors.extend(bad("body", "No control characters other than line breaks"));
    }
    if errors.is_empty() {
        Ok(MacroDef { id: def.id.clone(), name: name.to_string(), body: def.body.clone(), quick: def.quick })
    } else {
        Err(errors)
    }
}

/// The macros a profile starts with: short, friendly, and useful on HF.
pub const DEFAULTS: [(&str, &str); 3] = [
    ("Station info", "Station info: <NAME>, <QTH> <LOC>. Rig <RIG>, <PWR>, antenna <ANT>."),
    ("Greeting", "Hello <CALL>, this is <MYCALL>, <NAME>. Good to hear you."),
    ("Closing", "Thanks for the contact <CALL>. 73 from <NAME>, <MYCALL>."),
];
