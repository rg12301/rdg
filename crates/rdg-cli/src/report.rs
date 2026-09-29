//! Everything rdg tells its caller about a run, in one place: printed to stderr as it
//! happens (`--format text`), or collected and printed as a single JSON object on stdout
//! at the end (`--format json`) for agents and scripts to parse.

use std::fmt::Write as _;

use crate::validate::{Diagnostic, Level};

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

#[derive(Default)]
struct Item {
    path: Option<String>,
    message: String,
    hint: Option<String>,
}

pub struct Report {
    format: Format,
    errors: Vec<Item>,
    warnings: Vec<Item>,
    anomalies: Vec<(String, String)>,
    layout: Option<String>,
    output: Option<String>,
}

impl Report {
    pub fn new(format: Format) -> Self {
        Report { format, errors: Vec::new(), warnings: Vec::new(), anomalies: Vec::new(), layout: None, output: None }
    }

    pub fn is_json(&self) -> bool {
        self.format == Format::Json
    }

    /// Progress / informational line (text mode only).
    pub fn note(&self, line: &str) {
        if self.format == Format::Text {
            eprintln!("{line}");
        }
    }

    pub fn diagnostic(&mut self, d: &Diagnostic) {
        if self.format == Format::Text {
            eprintln!("{d}");
        }
        let item = Item { path: Some(d.path.clone()), message: d.message.clone(), hint: d.hint.clone() };
        match d.level {
            Level::Error => self.errors.push(item),
            Level::Warning => self.warnings.push(item),
        }
    }

    pub fn warning(&mut self, message: &str) {
        if self.format == Format::Text {
            eprintln!("⚠  {message}");
        }
        self.warnings.push(Item { message: message.into(), ..Default::default() });
    }

    pub fn anomaly(&mut self, kind: &str, message: &str) {
        self.anomalies.push((kind.into(), message.into()));
    }

    pub fn layout(&mut self, name: &str) {
        self.layout = Some(name.into());
    }

    pub fn written(&mut self, path: &str) {
        self.note(&format!("✓ Diagram written to {path}"));
        self.output = Some(path.into());
    }

    /// Final JSON object (json mode), for a run that ended in `error` or succeeded.
    pub fn finish(mut self, error: Option<&anyhow::Error>) {
        if self.format != Format::Json {
            return;
        }
        // Validation errors are already listed; otherwise the error that ended the run
        // (a parse error, an unreadable file, …) is the one to report.
        if let (Some(e), true) = (error, self.errors.is_empty()) {
            // A parse error names its location as "nodes[0]: unknown field …".
            let msg = e.root_cause().to_string();
            let (path, message) = match msg.split_once(": ") {
                Some((p, m)) if !p.contains(' ') && (p.contains('[') || p.contains('.')) => (Some(p.to_string()), m.to_string()),
                _ => (None, msg.clone()),
            };
            let (message, hint) = match message.split_once(" — ") {
                Some((m, h)) => (m.to_string(), Some(h.to_string())),
                None => (message, None),
            };
            self.errors.push(Item { path, message, hint });
        }
        let ok = error.is_none();
        let mut s = String::from("{");
        let _ = write!(s, "\"ok\":{ok},\"output\":{}", opt(&self.output));
        let _ = write!(s, ",\"layout\":{}", opt(&self.layout));
        for (name, items) in [("errors", &self.errors), ("warnings", &self.warnings)] {
            let _ = write!(s, ",\"{name}\":[");
            for (k, i) in items.iter().enumerate() {
                if k > 0 {
                    s.push(',');
                }
                let _ = write!(s, "{{\"path\":{},\"message\":{},\"hint\":{}}}", opt(&i.path), esc(&i.message), opt(&i.hint));
            }
            s.push(']');
        }
        s.push_str(",\"anomalies\":[");
        for (k, (kind, m)) in self.anomalies.iter().enumerate() {
            if k > 0 {
                s.push(',');
            }
            let _ = write!(s, "{{\"kind\":{},\"message\":{}}}", esc(kind), esc(m));
        }
        s.push_str("]}");
        println!("{s}");
    }
}

fn opt(v: &Option<String>) -> String {
    v.as_deref().map_or("null".into(), esc)
}

/// A JSON string literal.
pub fn esc(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
