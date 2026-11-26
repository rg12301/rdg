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
    closest_match(word, candidates).filter(|c| !c.eq_ignore_ascii_case(word)).map(|c| format!("did you mean `{c}`?"))
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

    // --- scale ---------------------------------------------------------------
    // Above about 25 nodes or 1.5 edges per node the arrows get long and tangled
    // whatever the layout does; the things to cut or split are the hubs (nodes with
    // many arrows) and the collectors (one node every group reports to).
    if payload.resolved_diagram_type() != "sequence" && !payload.nodes.is_empty() {
        let n = payload.nodes.len();
        let e = payload.edges.len();
        let density = e as f64 / n as f64;
        let mut degree: HashMap<&str, usize> = HashMap::new();
        for ed in &payload.edges {
            for id in ed.from.iter().chain(ed.to.iter()) {
                *degree.entry(id).or_default() += 1;
            }
        }
        let mut hubs: Vec<(usize, &str)> = degree.iter().filter(|&(_, &d)| d >= 5).map(|(&id, &d)| (d, id)).collect();
        hubs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        let group_of: HashMap<&str, &str> = payload.groups.iter().flat_map(|g| g.nodes.iter().map(move |n| (n.as_str(), g.id.as_str()))).collect();
        let mut feeders: HashMap<&str, HashSet<&str>> = HashMap::new();
        for ed in &payload.edges {
            for (f, t) in ed.from.iter().flat_map(|f| ed.to.iter().map(move |t| (f, t))) {
                let Some(g) = group_of.get(f) else { continue };
                feeders.entry(t).or_default().insert(g);
            }
        }
        let mut collectors: Vec<&str> = feeders.iter().filter(|(_, gs)| gs.len() >= 3).map(|(&id, _)| id).collect();
        collectors.sort_unstable();
        let mut who = Vec::new();
        if !hubs.is_empty() {
            who.push(format!("hubs: {}", hubs.iter().take(4).map(|(d, id)| format!("{id} ({d} arrows)")).collect::<Vec<_>>().join(", ")));
        }
        if !collectors.is_empty() {
            who.push(format!("collectors (fed from ≥ 3 groups): {}", collectors.iter().take(4).copied().collect::<Vec<_>>().join(", ")));
        }
        let who = if who.is_empty() { None } else { Some(who.join("; ")) };
        if n > 40 || density > 2.0 {
            push(
                Level::Warning,
                "nodes".into(),
                format!("{n} nodes and {e} edges ({density:.1} per node): a diagram this size does not read as a design"),
                Some(match &who {
                    Some(w) => format!("split it into an overview (groups as nodes) and one diagram per group; {w}"),
                    None => "split it into an overview (groups as nodes) and one diagram per group".into(),
                }),
            );
        } else if n > 25 || density > 1.5 {
            push(
                Level::Warning,
                "nodes".into(),
                format!("{n} nodes and {e} edges ({density:.1} per node): above the 25-node, 1.5-per-node budget, expect long arrows"),
                Some(match &who {
                    Some(w) => format!("cut the arrows that do not tell the story, or split the diagram; {w}"),
                    None => "cut the arrows that do not tell the story, or split the diagram".into(),
                }),
            );
        }
    }

    // --- edge endpoints ---------------------------------------------------------
    for (i, e) in payload.edges.iter().enumerate() {
        for (field, end) in [("from", &e.from), ("to", &e.to)] {
            for id in end.iter() {
                if !node_set.contains(id) {
                    push(
                        Level::Error,
                        format!("edges[{i}].{field}"),
                        format!("unknown node id `{id}`"),
                        did_you_mean(id, node_ids.iter().copied()),
                    );
                }
            }
        }
        if e.from.len() > 1 && e.to.len() > 1 {
            push(
                Level::Error,
                format!("edges[{i}]"),
                "both ends list several nodes".into(),
                Some("only one end of an arrow may be a list: write one arrow per node on the other side".into()),
            );
        }
        for (field, end) in [("from", &e.from), ("to", &e.to)] {
            let mut seen = std::collections::HashSet::new();
            if let Some(dup) = end.iter().find(|id| !seen.insert(*id)) {
                push(Level::Warning, format!("edges[{i}].{field}"), format!("node id `{dup}` is listed twice"), None);
            }
        }
    }

    // --- groups: members (nodes or nested groups), nesting --------------------------
    let group_ids: Vec<&str> = payload.groups.iter().map(|g| g.id.as_str()).collect();
    // A node reaches another when a chain of arrows leads from it to the other.
    let reaches = |from: &str, to: &str| -> bool {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![from];
        while let Some(u) = stack.pop() {
            if u == to {
                return true;
            }
            if seen.insert(u) {
                for e in &payload.edges {
                    if e.from.iter().any(|f| f == u) {
                        stack.extend(e.to.iter());
                    }
                }
            }
        }
        false
    };
    for (i, set) in payload.same_rank.iter().enumerate() {
        if let Some((a, b)) = set.iter().flat_map(|a| set.iter().map(move |b| (a, b))).find(|(a, b)| a != b && reaches(a, b)) {
            push(
                Level::Warning,
                format!("same_rank[{i}]"),
                format!("`{a}` leads to `{b}` by a chain of arrows, so they cannot share a rank; the list is ignored"),
                Some("level only nodes that do not feed one another".into()),
            );
        }
        for (j, id) in set.iter().enumerate() {
            if !node_set.contains(id.as_str()) && !group_ids.contains(&id.as_str()) {
                push(
                    Level::Error,
                    format!("same_rank[{i}][{j}]"),
                    format!("unknown node or group id `{id}`"),
                    did_you_mean(id, node_ids.iter().chain(&group_ids).copied()),
                );
            }
        }
    }
    for (i, g) in payload.groups.iter().enumerate() {
        for (j, id) in g.nodes.iter().enumerate() {
            if !node_set.contains(id.as_str()) && !group_ids.contains(&id.as_str()) {
                push(
                    Level::Error,
                    format!("groups[{i}].nodes[{j}]"),
                    format!("unknown node or group id `{id}`"),
                    did_you_mean(id, node_ids.iter().chain(&group_ids).copied()),
                );
            }
        }
        if let Some(p) = g.parent.as_deref().filter(|p| !group_ids.contains(p)) {
            push(Level::Error, format!("groups[{i}].parent"), format!("unknown group id `{p}`"), did_you_mean(p, group_ids.iter().copied()));
        }
    }
    for issue in payload.group_tree().issues {
        // (An unknown `parent` is reported above, with a suggestion.)
        if !issue.message.starts_with("unknown group id") {
            push(if issue.error { Level::Error } else { Level::Warning }, issue.path, issue.message, None);
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
        if !raw.label.trim().is_empty() && raw.title.as_deref().is_some_and(|t| !t.trim().is_empty()) {
            push(
                Level::Warning,
                format!("nodes[{i}].label"),
                "`label` is ignored when `title` is set".into(),
                Some("keep `title` (and `subtitle` / `description`), drop `label`".into()),
            );
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
            if !rdg_icons::has_icon(icon) {
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
        if let Some(problem) = raw.step.as_deref().and_then(rdg_schema::step_label_problem) {
            push(Level::Error, format!("edges[{i}].step"), problem.into(), None);
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
    // A label on most of the arrows says nothing: the legend or the node should say it
    // once, and only the exceptions need a label.
    if !is_sequence {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let labelled = payload.edges.iter().filter_map(|e| e.label.as_deref().map(str::trim).filter(|l| !l.is_empty())).inspect(|l| *counts.entry(l).or_default() += 1).count();
        let mut repeated: Vec<(&str, usize)> = counts.into_iter().filter(|&(_, n)| n >= 5 && n * 2 >= labelled).collect();
        repeated.sort();
        for (text, n) in repeated {
            out.push(Diagnostic {
                level: Level::Warning,
                path: "edges".into(),
                message: format!("the label \"{text}\" is on {n} of {labelled} labelled edges"),
                hint: Some("say it once (in the legend, a node's technology, or the diagram description) and label only the arrows that differ".into()),
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
                hint: Some("give the name as `title:` and the rest as `subtitle:` / `description:` — each wraps on its own, and a name is never split".into()),
            });
        }
    }
    for (i, n) in payload.nodes.iter().enumerate() {
        const SHAPES: [&str; 7] = ["card", "rounded", "cylinder", "ellipse", "diamond", "tile", "icon"];
        if let Some(s) = n.shape.as_deref().filter(|s| !SHAPES.contains(&s.to_ascii_lowercase().as_str())) {
            out.push(Diagnostic {
                level: Level::Warning,
                path: format!("nodes[{i}].shape"),
                message: format!("unknown shape `{s}`; the theme's shape is used"),
                hint: did_you_mean(s, SHAPES.iter().copied()),
            });
        }
        if let Some(d) = n.display.as_deref().filter(|d| !["tile", "card", "icon"].contains(&d.to_ascii_lowercase().as_str())) {
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
    fn warns_on_scale_and_names_the_hubs() {
        use rdg_schema::{EdgeDef, NodeDef};
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for i in 0..30 {
            nodes.push(NodeDef { id: format!("n{i}"), label: format!("N{i}"), ..Default::default() });
            if i > 0 {
                edges.push(EdgeDef { from: "n0".into(), to: format!("n{i}").into(), ..Default::default() });
            }
        }
        let payload = DiagramPayload { nodes, edges, ..Default::default() };
        let d: Vec<String> = validate(&payload).iter().map(ToString::to_string).collect();
        let scale = d.iter().find(|m| m.contains("30 nodes")).expect("a scale warning");
        assert!(scale.contains("n0 (29 arrows)"), "{scale}");
        let small = DiagramPayload { nodes: vec![NodeDef { id: "a".into(), label: "A".into(), ..Default::default() }], ..Default::default() };
        assert!(validate(&small).iter().all(|d| !d.message.contains("nodes and")));
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

    #[test]
    fn nested_groups_and_branch_steps_are_valid() {
        let d = diags(
            "nodes:\n  - {id: a, title: A}\n  - {id: b, title: B}\n  - {id: c, title: C}\n\
             groups:\n  - {id: outer, label: Outer, nodes: [a, inner]}\n  - {id: inner, label: Inner, nodes: [b]}\n  - {id: deep, label: Deep, parent: inner, nodes: [c]}\n\
             edges:\n  - {from: a, to: b, step: 1}\n  - {from: a, to: c, step: 2a}\n  - {from: b, to: c, step: \"2b\"}\n",
        );
        assert!(d.is_empty(), "{d:#?}");
    }

    #[test]
    fn reports_group_and_step_mistakes() {
        let d = diags(
            "nodes:\n  - {id: a, label: A, title: Alpha}\n  - {id: b, label: B}\n  - {id: c, label: C}\n\
             groups:\n  - {id: x, label: X, nodes: [a, b, innr]}\n  - {id: y, label: Y, nodes: [b, c]}\n  - {id: inner, label: I, parent: nope, nodes: [c]}\n\
             edges:\n  - {from: a, to: b, step: b3}\n",
        );
        assert!(d.iter().any(|m| m.contains("groups[0].nodes[2]") && m.contains("did you mean `inner`?")), "{d:#?}");
        assert!(d.iter().any(|m| m.starts_with("warning: groups[1].nodes[0]") && m.contains("not nested")), "{d:#?}");
        assert!(d.iter().any(|m| m.starts_with("error: groups[2].parent")), "{d:#?}");
        assert!(d.iter().any(|m| m.starts_with("error: edges[0].step")), "{d:#?}");
        assert!(d.iter().any(|m| m.starts_with("warning: nodes[0].label") && m.contains("ignored")), "{d:#?}");
    }
}
