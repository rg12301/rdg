//! Semantic checks on a parsed payload, run before anything is laid out.
//!
//! Parsing already rejects unknown *keys*; this catches wrong *values* — an edge to a
//! node id that doesn't exist, a group listing a missing node, a node `type`, edge style
//! or icon name the renderers don't know. It reports every problem at once, each with a
//! YAML path and (where one is close) a "did you mean", so an agent can fix them all in
//! a single edit instead of discovering them one failed run at a time.

use std::collections::{HashMap, HashSet};
use std::fmt;

use rdg_schema::{DiagramPayload, closest_match};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Blocks rendering.
    Error,
    /// Renders, but probably not as intended.
    Warning,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: Level,
    /// YAML location, e.g. `edges[3].to`.
    pub path: String,
    pub message: String,
    pub hint: Option<String>,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        write!(f, "{level}: {}: {}", self.path, self.message)?;
        if let Some(h) = &self.hint {
            write!(f, " — {h}")?;
        }
        Ok(())
    }
}

fn did_you_mean<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    closest_match(word, candidates).map(|c| format!("did you mean `{c}`?"))
}

pub fn validate(payload: &DiagramPayload) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut push = |level, path: String, message: String, hint: Option<String>| {
        out.push(Diagnostic { level, path, message, hint });
    };

    // --- ids ------------------------------------------------------------------
    let mut seen: HashMap<&str, String> = HashMap::new();
    for (i, n) in payload.nodes.iter().enumerate() {
        let path = format!("nodes[{i}].id");
        if let Some(first) = seen.get(n.id.as_str()) {
            push(Level::Error, path, format!("duplicate id `{}` (first used at {first})", n.id), None);
        } else {
            seen.insert(&n.id, path);
        }
    }
    for (i, g) in payload.groups.iter().enumerate() {
        let path = format!("groups[{i}].id");
        if let Some(first) = seen.get(g.id.as_str()) {
            push(Level::Error, path, format!("duplicate id `{}` (first used at {first})", g.id), None);
        } else {
            seen.insert(&g.id, path);
        }
    }
    let node_ids: Vec<&str> = payload.nodes.iter().map(|n| n.id.as_str()).collect();
    let node_set: HashSet<&str> = node_ids.iter().copied().collect();

    // --- edge endpoints ---------------------------------------------------------
    for (i, e) in payload.edges.iter().enumerate() {
        for (field, id) in [("from", &e.from), ("to", &e.to)] {
            if !node_set.contains(id.as_str()) {
                push(
                    Level::Error,
                    format!("edges[{i}].{field}"),
                    format!("unknown node id `{id}`"),
                    did_you_mean(id, node_ids.iter().copied()),
                );
            }
        }
    }

    // --- group membership ---------------------------------------------------------
    let mut member_of: HashMap<&str, &str> = HashMap::new();
    for (i, g) in payload.groups.iter().enumerate() {
        for (j, id) in g.nodes.iter().enumerate() {
            if !node_set.contains(id.as_str()) {
                push(
                    Level::Error,
                    format!("groups[{i}].nodes[{j}]"),
                    format!("unknown node id `{id}`"),
                    did_you_mean(id, node_ids.iter().copied()),
                );
            } else if let Some(other) = member_of.insert(id, &g.id) {
                push(
                    Level::Warning,
                    format!("groups[{i}].nodes[{j}]"),
                    format!("node `{id}` is already in group `{other}`; a node can belong to one group"),
                    None,
                );
            }
        }
    }

    // --- node values ----------------------------------------------------------------
    let icon_keys = rdg_icons::icon_keys();
    let preset_names: Vec<&str> = payload.node_styles.keys().map(String::as_str).collect();
    for (i, raw) in payload.nodes.iter().enumerate() {
        if let Some(class) = raw.class.as_deref() {
            if !payload.node_styles.contains_key(class) {
                push(
                    Level::Error,
                    format!("nodes[{i}].class"),
                    format!("no `node_styles` preset named `{class}`"),
                    did_you_mean(class, preset_names.iter().copied()),
                );
            }
        }
        let n = payload.effective_node(raw);
        let ty = n.node_type.to_ascii_lowercase();
        if let Some(lang) = n.language.as_deref().filter(|l| !l.trim().is_empty()) {
            if rdg_icons::detect_language(Some(lang), None, "", None).is_none() {
                push(
                    Level::Warning,
                    format!("nodes[{i}].language"),
                    format!("no icon for language `{lang}`; it will be omitted"),
                    did_you_mean(lang, icon_keys.iter().copied()),
                );
            }
        }
        if let Some(db) = n.db_type.as_deref().filter(|d| !d.trim().is_empty()) {
            if rdg_icons::detect_database(Some(db), None, "", &ty).is_none() {
                push(
                    Level::Warning,
                    format!("nodes[{i}].db_type"),
                    format!("no icon for database `{db}`; it will be omitted"),
                    did_you_mean(db, icon_keys.iter().copied()),
                );
            }
        }
        if let Some(icon) = n.icon.as_deref().filter(|d| !d.trim().is_empty()) {
            if rdg_icons::get_icon(icon).is_none() {
                push(
                    Level::Warning,
                    format!("nodes[{i}].icon"),
                    format!("unknown icon `{icon}`; it will be omitted"),
                    did_you_mean(icon, icon_keys.iter().copied()),
                );
            }
        }
        if let Some(p) = n.provider.as_deref().filter(|d| !d.trim().is_empty()) {
            if rdg_icons::detect_provider(p).is_none() {
                push(
                    Level::Warning,
                    format!("nodes[{i}].provider"),
                    format!("no icon for provider `{p}` (known: aws, gcp); it will be omitted"),
                    None,
                );
            }
        }
    }

    // --- sequence script ------------------------------------------------------------
    if !payload.sequence.is_empty() && !payload.edges.is_empty() {
        push(Level::Error, "edges".into(), "a sequence diagram takes its messages from `sequence:` or `edges:`, not both".into(), Some("move the edges into `sequence:`".into()));
    }
    let ids: Vec<&str> = payload.nodes.iter().map(|n| n.id.as_str()).collect();
    fn check_steps(steps: &[rdg_schema::SeqStep], path: &str, ids: &[&str], push: &mut dyn FnMut(Level, String, String, Option<String>)) {
        for (i, s) in steps.iter().enumerate() {
            let p = format!("{path}[{i}]");
            if let Err(e) = s.kind() {
                push(Level::Error, p.clone(), e, None);
                continue;
            }
            let refs = [("from", s.from.as_deref()), ("to", s.to.as_deref()), ("left_of", s.left_of.as_deref()), ("right_of", s.right_of.as_deref())];
            for (field, id) in refs.into_iter().filter_map(|(f, v)| v.map(|v| (f, v))).chain(s.over.iter().map(|o| ("over", o.as_str()))) {
                if !ids.contains(&id) {
                    push(Level::Error, format!("{p}.{field}"), format!("unknown participant `{id}`"), did_you_mean(id, ids.iter().copied()));
                }
            }
            if s.note.is_some() && s.over.is_empty() && s.left_of.is_none() && s.right_of.is_none() {
                push(Level::Error, p.clone(), "a note needs `over`, `left_of` or `right_of`".into(), None);
            }
            check_steps(&s.steps, &format!("{p}.steps"), ids, push);
            for (j, b) in s.else_.iter().enumerate() {
                check_steps(&b.steps, &format!("{p}.else[{j}].steps"), ids, push);
            }
            for (j, b) in s.and.iter().enumerate() {
                check_steps(&b.steps, &format!("{p}.and[{j}].steps"), ids, push);
            }
        }
    }
    check_steps(&payload.sequence, "sequence", &ids, &mut push);

    // --- edge values ----------------------------------------------------------------
    let edge_presets: Vec<&str> = payload.edge_styles.keys().map(String::as_str).collect();
    for (i, raw) in payload.edges.iter().enumerate() {
        if let Some(class) = raw.class.as_deref() {
            if !payload.edge_styles.contains_key(class) {
                push(
                    Level::Error,
                    format!("edges[{i}].class"),
                    format!("no `edge_styles` preset named `{class}`"),
                    did_you_mean(class, edge_presets.iter().copied()),
                );
            }
        }
        let e = payload.effective_edge(raw);
        if let Some(style) = e.edge_style.as_deref() {
            let s = style.to_ascii_lowercase();
            if !rdg_render_core::style::EDGE_STYLES.contains(&s.as_str()) {
                push(
                    Level::Warning,
                    format!("edges[{i}].edge_style"),
                    format!("unknown edge_style `{style}` renders as `flow`"),
                    did_you_mean(&s, rdg_render_core::style::EDGE_STYLES.iter().copied()),
                );
            }
        }
    }

    out
}

/// Checks that need the resolved theme: every `category` it names, and `display` values.
pub fn validate_against_theme(payload: &DiagramPayload, theme: &rdg_render_core::theme::Theme) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let cats: Vec<&str> = theme.categories.keys().map(String::as_str).collect();
    let mut check = |path: String, cat: Option<&str>| {
        if let Some(c) = cat.filter(|c| !theme.categories.contains_key(*c)) {
            out.push(Diagnostic {
                level: Level::Warning,
                path,
                message: format!("unknown category `{c}`; the type's category is used"),
                hint: did_you_mean(c, cats.iter().copied()).or_else(|| Some(format!("known: {}", cats.join(", ")))),
            });
        }
    };
    for (i, n) in payload.nodes.iter().enumerate() {
        check(format!("nodes[{i}].category"), n.category.as_deref());
    }
    for (i, g) in payload.groups.iter().enumerate() {
        check(format!("groups[{i}].category"), g.category.as_deref());
    }
    // Long single lines: rdg wraps them, but an agent that splits them itself gets
    // better breaks and learns the house style.
    let edge_max = theme.edge_label.wrap_chars;
    // (Sequence messages run along wide horizontal arrows; one line is right there.)
    let is_sequence = payload.resolved_diagram_type() == "sequence";
    for (i, e) in payload.edges.iter().enumerate().filter(|_| !is_sequence) {
        if let Some(line) = e.label.as_deref().and_then(|l| l.lines().find(|l| l.chars().count() > edge_max)) {
            out.push(Diagnostic {
                level: Level::Warning,
                path: format!("edges[{i}].label"),
                message: format!("{} characters on one line (\"{line}\")", line.chars().count()),
                hint: Some(format!("split it with \\n into lines of ≤ {edge_max} characters")),
            });
        }
    }
    // Node types: the renderers' own, plus any the theme gives a category, shape or
    // actor figure.
    let mut known: Vec<&str> = rdg_render_core::style::NODE_TYPES.to_vec();
    known.extend(theme.type_categories.keys().map(String::as_str));
    known.extend(theme.shapes.keys().map(String::as_str));
    known.extend(theme.sequence.actor_types.iter().map(String::as_str));
    for (i, raw) in payload.nodes.iter().enumerate() {
        let n = payload.effective_node(raw);
        let ty = n.node_type.to_ascii_lowercase();
        if !known.contains(&ty.as_str()) {
            out.push(Diagnostic {
                level: Level::Warning,
                path: format!("nodes[{i}].type"),
                message: format!("unknown node type `{}` renders as a plain card", n.node_type),
                hint: did_you_mean(&ty, known.iter().copied()).or_else(|| Some("see `rdg --help` NODE TYPES".into())),
            });
        }
    }
    // Colour means category: a hand-picked colour breaks that code (and the legend).
    if theme.custom_colors {
        let cats = cats.join(", ");
        for (i, raw) in payload.nodes.iter().enumerate() {
            if payload.effective_node(raw).color.is_some() {
                out.push(Diagnostic {
                    level: Level::Warning,
                    path: format!("nodes[{i}].color"),
                    message: "a custom colour replaces the category colour, so it no longer says what the node is".into(),
                    hint: Some(format!("drop it, or set `category:` ({cats}) to colour by meaning")),
                });
            }
        }
        for (i, g) in payload.groups.iter().enumerate() {
            if g.color.is_some() {
                out.push(Diagnostic {
                    level: Level::Warning,
                    path: format!("groups[{i}].color"),
                    message: "a coloured container competes with the colours of the nodes inside it".into(),
                    hint: Some("drop it (containers are neutral), or set `category:` when the whole group is one kind".into()),
                });
            }
        }
        for (i, raw) in payload.edges.iter().enumerate() {
            if payload.effective_edge(raw).color.is_some() {
                out.push(Diagnostic {
                    level: Level::Warning,
                    path: format!("edges[{i}].color"),
                    message: "a custom colour hides what the arrow means".into(),
                    hint: Some("use `edge_style:` (async, data, auth, error, …) — its colour carries the meaning".into()),
                });
            }
        }
    }
    const NODE_LINE_MAX: usize = 30;
    for (i, n) in payload.nodes.iter().enumerate() {
        if let Some(line) = n.label.lines().find(|l| l.chars().count() > NODE_LINE_MAX) {
            out.push(Diagnostic {
                level: Level::Warning,
                path: format!("nodes[{i}].label"),
                message: format!("{} characters on one line (\"{line}\")", line.chars().count()),
                hint: Some(format!("split it with \\n into lines of ≤ {NODE_LINE_MAX} characters, or move detail to `metadata`")),
            });
        }
    }
    for (i, n) in payload.nodes.iter().enumerate() {
        if let Some(d) = n.display.as_deref().filter(|d| !["card", "icon"].contains(&d.to_ascii_lowercase().as_str())) {
            out.push(Diagnostic {
                level: Level::Warning,
                path: format!("nodes[{i}].display"),
                message: format!("unknown display `{d}`"),
                hint: Some("use `card` or `icon`".into()),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diags(yaml: &str) -> Vec<String> {
        let p = DiagramPayload::from_yaml(yaml).unwrap();
        let theme = rdg_render_core::theme::Theme::default();
        validate(&p).iter().chain(&validate_against_theme(&p, &theme)).map(ToString::to_string).collect()
    }

    #[test]
    fn checks_sequence_steps() {
        let d = diags(
            "participants:\n  - {id: web, label: Web}\n  - {id: api, label: API}\n\
             sequence:\n  - {from: web, to: apii, label: call}\n  - {note: hi}\n  - alt: ok\n    steps:\n      - {from: api, to: web, note: both}\n",
        );
        assert!(d.iter().any(|m| m.contains("sequence[0].to") && m.contains("did you mean `api`?")), "{d:#?}");
        assert!(d.iter().any(|m| m.contains("sequence[1]") && m.contains("needs `over`")), "{d:#?}");
        assert!(d.iter().any(|m| m.contains("sequence[2].steps[0]") && m.contains("exactly one")), "{d:#?}");
    }

    #[test]
    fn reports_every_bad_reference_with_suggestions() {
        let d = diags(
            "nodes:\n  - {id: auth_api, label: A}\n  - {id: redis, label: R}\n\
             groups:\n  - {id: g, label: G, nodes: [auth_api, rediss]}\n\
             edges:\n  - {from: auth_apii, to: redis}\n  - {from: auth_api, to: postgres}\n",
        );
        assert_eq!(d.len(), 3, "{d:#?}");
        assert!(d.iter().any(|m| m.contains("groups[0].nodes[1]") && m.contains("did you mean `redis`?")));
        assert!(d.iter().any(|m| m.contains("edges[0].from") && m.contains("did you mean `auth_api`?")));
        assert!(d.iter().any(|m| m.starts_with("error: edges[1].to: unknown node id `postgres`")));
    }

    #[test]
    fn warns_on_unknown_types_styles_and_icons() {
        let d = diags(
            "nodes:\n  - {id: a, label: A, type: servce, language: typscript}\n  - {id: b, label: B, db_type: postgress}\n\
             edges:\n  - {from: a, to: b, edge_style: asynch}\n",
        );
        assert!(d.iter().any(|m| m.contains("nodes[0].type") && m.contains("did you mean `service`?")), "{d:#?}");
        assert!(d.iter().any(|m| m.contains("nodes[0].language") && m.contains("did you mean `typescript`?")), "{d:#?}");
        assert!(d.iter().any(|m| m.contains("nodes[1].db_type") && m.contains("did you mean `postgres")), "{d:#?}");
        assert!(d.iter().any(|m| m.contains("edges[0].edge_style") && m.contains("did you mean `async`?")), "{d:#?}");
    }

    #[test]
    fn clean_payload_has_no_diagnostics() {
        assert!(diags("nodes:\n  - {id: a, label: A, type: service, language: rust}\n  - {id: b, label: B, type: database, db_type: postgres}\nedges:\n  - {from: a, to: b, edge_style: data}\n").is_empty());
    }
}
