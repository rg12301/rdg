//! `rdg` (Render Diagram) — binary entry point.
//!
//! Parses CLI arguments, reads the YAML payload, runs the pipeline, and writes
//! the output file. All heavy lifting is in the `rdg-schema`/`rdg-graph`/`rdg-layout`/
//! `rdg-render-drawio`/`rdg-render-svg` library crates.

mod guide;
mod report;
mod validate;

use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};

use rdg_graph::build_graph;
use rdg_layout::{DesignTokens, LayoutConfig};
use rdg_render_core::review::compute_reviewed_layout;
use rdg_render_drawio::render_drawio;
use rdg_render_svg::render_svg;
use rdg_schema::DiagramPayload;

use report::{Format, Report};


// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// rdg (Render Diagram) — compiles a YAML diagram description into draw.io XML or SVG.
#[derive(Parser, Debug)]
#[command(
    name = "rdg",
    version,
    about = "Compile a YAML diagram description (nodes, edges, groups) into a laid-out .drawio or .svg file.",
    long_about = guide::LONG_ABOUT,
    after_help = "`rdg --help` shows the YAML format; `rdg --guide all` has every detail."
)]
struct Cli {
    /// Input YAML file; stdin if omitted or '-'.
    #[arg(short, long, value_name = "FILE")]
    input: Option<String>,

    /// Input YAML file given positionally (same as -i).
    #[arg(value_name = "INPUT", conflicts_with = "input")]
    input_pos: Option<String>,

    /// Output file; the extension picks the format (.drawio or .svg).
    #[arg(short, long, default_value = "output.drawio", value_name = "FILE")]
    output: String,

    /// Validate the YAML only: report every error and warning, write nothing.
    #[arg(long)]
    check: bool,

    /// How results and diagnostics are reported: text on stderr, or one JSON object on stdout.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,

    /// Print a detailed guide on one topic and exit.
    #[arg(long, value_enum, value_name = "TOPIC")]
    guide: Option<guide::Topic>,

    /// Print every icon key (for `icon:`, `language:`, `db_type:`) and exit.
    #[arg(long)]
    list_icons: bool,

    /// Print a complete, commented YAML template and exit.
    #[arg(long)]
    example: bool,

    /// Print the JSON Schema of the YAML format and exit.
    #[arg(long)]
    schema: bool,

    /// Layout engine; `auto` picks from the graph's shape and reports its choice.
    #[arg(short, long, value_enum, default_value_t = LayoutEngine::Auto)]
    layout: LayoutEngine,

    /// Theme: a built-in name (light, dark, mono-light, mono-dark, classic) or a theme
    /// YAML file. Wins over the YAML `theme:` name; a YAML override mapping still applies on top.
    #[arg(short, long, value_name = "THEME|FILE")]
    theme: Option<String>,

    /// List the built-in themes and exit.
    #[arg(long)]
    list_themes: bool,

    /// Print a theme as YAML (a starting point for a custom one) and exit.
    #[arg(long, value_name = "THEME")]
    print_theme: Option<String>,

    /// Flow direction; the YAML `direction:` wins.
    #[arg(long, value_enum, default_value_t = CliDirection::Tb)]
    direction: CliDirection,

    /// Gap between ranks in px (default: proportional to node size); YAML `rank_spacing:` wins.
    #[arg(long, value_name = "PIXELS")]
    rank_spacing: Option<u32>,

    /// Gap between neighbouring nodes in px (default: proportional); YAML `node_spacing:` wins.
    #[arg(long, value_name = "PIXELS")]
    node_spacing: Option<u32>,

    /// SVG engine: auto (draw.io CLI if installed, else native), drawio, native.
    #[arg(long, default_value = "auto")]
    svg_engine: String,

    /// Exit 1 if layout anomalies (overlaps, arrows through nodes, …) remain; the file is still written.
    #[arg(long)]
    strict: bool,

    /// Skip the final polish pass (small port/waypoint clean-ups).
    #[arg(long)]
    no_polish: bool,

    /// YAML file overriding design tokens (spacing unit, font size, thresholds); see `rdg_layout::DesignTokens`.
    #[arg(long, value_name = "FILE")]
    design_config: Option<String>,
}

/// Diagram flow direction.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum CliDirection {
    /// Top-to-bottom flow.
    Tb,
    /// Left-to-right flow.
    Lr,
}

/// Layout framework to use for spatial positioning.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutEngine {
    /// Pick from the graph's shape (default).
    Auto,
    /// Layered: hierarchies, flows, grouped architectures.
    Sugiyama,
    /// Force-directed: dense webs.
    Force,
    /// Compound spring embedder: disconnected components.
    #[value(name = "fcose")]
    FCose,
}

impl LayoutEngine {
    /// The forced framework this flag value maps to, or `None` for `Auto` (meaning:
    /// let the topology dispatcher's own choice stand).
    fn as_framework(&self) -> Option<rdg_dispatch::LayoutFramework> {
        match self {
            LayoutEngine::Auto => None,
            LayoutEngine::Sugiyama => Some(rdg_dispatch::LayoutFramework::Sugiyama),
            LayoutEngine::Force => Some(rdg_dispatch::LayoutFramework::ForceDirected),
            LayoutEngine::FCose => Some(rdg_dispatch::LayoutFramework::FCose),
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();
    let mut report = Report::new(cli.format);
    let outcome = run(&cli, &mut report);
    let json = report.is_json();
    match outcome {
        Ok(code) => {
            report.finish(None);
            std::process::exit(code);
        }
        Err(e) => {
            if !json {
                eprintln!("Error: {e:?}");
            }
            report.finish(Some(&e));
            std::process::exit(1);
        }
    }
}

/// The whole pipeline; returns the exit code on success.
fn run(cli: &Cli, report: &mut Report) -> Result<i32> {
    // --- Informational flags --------------------------------------------------
    if cli.example {
        print!("{}", DiagramPayload::example_yaml());
        return Ok(0);
    }
    if cli.schema {
        print!("{}", DiagramPayload::json_schema());
        return Ok(0);
    }
    if let Some(topic) = cli.guide {
        println!("{}", guide::text(topic));
        return Ok(0);
    }
    if cli.list_themes {
        for (name, src) in rdg_render_core::theme::BUILTIN {
            let about = src.lines().next().unwrap_or("").trim_start_matches('#').trim();
            println!("{name:<12} {about}");
        }
        return Ok(0);
    }
    if let Some(name) = &cli.print_theme {
        print!("{}", resolve_theme(Some(name), None)?.to_yaml());
        return Ok(0);
    }
    if cli.list_icons {
        let keys = rdg_icons::icon_keys();
        if report.is_json() {
            let list: Vec<String> = keys.iter().map(|k| report::esc(k)).collect();
            println!("[{}]", list.join(","));
        } else {
            println!("{}", keys.join("\n"));
        }
        std::process::exit(0);
    }

    // --- 1. Read YAML input -------------------------------------------------
    let yaml = match cli.input.as_deref().or(cli.input_pos.as_deref()) {
        None | Some("-") => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("failed to read YAML from stdin")?;
            buf
        }
        Some(path) => fs::read_to_string(path)
            .with_context(|| format!("failed to read input file '{path}'"))?,
    };

    // --- 2. Deserialise payload ---------------------------------------------
    let payload = DiagramPayload::from_yaml(&yaml).context(
        "YAML payload does not match the rdg schema — see `rdg --help` or `rdg --guide schema`",
    )?;

    // --- 2b. Semantic checks: report every problem at once, fail on errors ---------
    let diagnostics = validate::validate(&payload);
    for d in &diagnostics {
        report.diagnostic(d);
    }
    let errors = diagnostics.iter().filter(|d| d.level == validate::Level::Error).count();
    if errors > 0 {
        anyhow::bail!("{errors} error(s) in the YAML payload — fix them and re-run (nothing was written)");
    }

    // --- 2c. Theme: CLI name/file, else the YAML's; YAML overrides layered on top -------
    let theme = resolve_theme(cli.theme.as_deref(), payload.theme.as_ref())
        .map_err(|e| anyhow::anyhow!("theme: {e:#}"))?;
    for d in validate::validate_against_theme(&payload, &theme) {
        report.diagnostic(&d);
    }

    // `rdg-schema`'s `Direction` has no dependency on `rdg-layout` (to avoid a crate
    // cycle), so the CLI is what maps between the schema's and the layout engine's
    // otherwise-identical direction types.
    let cli_direction = match cli.direction {
        CliDirection::Tb => rdg_layout::LayoutDirection::TopToBottom,
        CliDirection::Lr => rdg_layout::LayoutDirection::LeftToRight,
    };
    let direction = match payload.resolved_direction() {
        Some(rdg_schema::Direction::TopToBottom) => rdg_layout::LayoutDirection::TopToBottom,
        Some(rdg_schema::Direction::LeftToRight) => rdg_layout::LayoutDirection::LeftToRight,
        None => cli_direction,
    };

    // Base design tokens: built-in defaults, optionally overridden wholesale by
    // `--design-config` (a YAML file setting any subset of `DesignTokens`'s fields —
    // see that struct's own docs). This is the one thing in the pipeline read from an
    // external file rather than computed; everything else derives from it.
    let mut tokens = load_design_tokens(cli.design_config.as_deref())?;
    if cli.no_polish {
        tokens.polish_enabled = false;
    }
    // Text, icon and badge sizes come from the theme, so boxes fit what's drawn.
    theme.apply_to_tokens(&mut tokens);

    // --- 3. Build petgraph --------------------------------------------------
    // Moved ahead of the spacing block below: computing a proportional
    // `rank_spacing`/`node_spacing` default needs this diagram's own node sizes,
    // which needs the compiled graph. `build_graph` only depends on `payload`, so
    // nothing else here needed to move with it.
    let mut compiled = build_graph(&payload)?;
    rdg_render_core::look::prepare_graph(&theme, &mut compiled);
    if compiled.had_cycles {
        report.warning("cycles in the graph were broken automatically (some arrows point against the flow)");
    }
    if cli.check {
        report.note(&format!(
            "✓ valid: {} node(s), {} {}, {} group(s){}",
            payload.nodes.len(),
            compiled.graph.edge_count(),
            if compiled.diagram_type == "sequence" { "message(s)" } else { "edge(s)" },
            payload.groups.len(),
            if diagnostics.is_empty() { String::new() } else { format!(", {} warning(s)", diagnostics.len()) }
        ));
        return Ok(0);
    }

    // --- Spacing: proportional by default, explicit override wins ------------
    // `rank_spacing`/`node_spacing`, when neither a YAML nor CLI value is given, are
    // computed here from this diagram's own average node size rather than a flat
    // pixel constant unrelated to it — see the CLI flags' own help text and
    // `DesignTokens::rank_spacing_fraction`/`node_spacing_fraction`.
    let default_node_w = LayoutConfig::default().node_width;
    let default_node_h = LayoutConfig::default().node_height;
    let (avg_node_w, avg_node_h) =
        rdg_layout::estimate_average_node_size(&compiled, default_node_w, default_node_h, &tokens);
    // rank_spacing runs along the flow direction (vertical in tb, horizontal in lr);
    // node_spacing runs across it — the proportion each uses swaps with direction,
    // same as the layout engines' own axis handling.
    let (along_dim, across_dim) = match direction {
        rdg_layout::LayoutDirection::TopToBottom => (avg_node_h, avg_node_w),
        rdg_layout::LayoutDirection::LeftToRight => (avg_node_w, avg_node_h),
    };
    let auto_rank_spacing = ((along_dim * tokens.rank_spacing_fraction).round() as u32).max(4);
    let auto_node_spacing = ((across_dim * tokens.node_spacing_fraction).round() as u32).max(4);

    // Flat `rank_spacing`/`node_spacing` (YAML or CLI flag) win over the grouped
    // `spacing.rank`/`spacing.node` object, which wins over the proportional default.
    let spacing = payload.spacing.as_ref();
    let rank_spacing = payload
        .rank_spacing
        .or_else(|| spacing.and_then(|s| s.rank))
        .or(cli.rank_spacing)
        .unwrap_or(auto_rank_spacing);
    let node_spacing = payload
        .node_spacing
        .or_else(|| spacing.and_then(|s| s.node))
        .or(cli.node_spacing)
        .unwrap_or(auto_node_spacing);
    let group_gap_x = spacing.and_then(|s| s.group_gap_x).unwrap_or(tokens.group_gap());
    let group_gap_y = spacing.and_then(|s| s.group_gap_y).unwrap_or(tokens.group_gap());
    let canvas = payload.canvas.as_ref();
    let margin = canvas.and_then(|c| c.margin);
    let margin_x = margin.unwrap_or(tokens.margin_x());
    let margin_y = margin.unwrap_or(tokens.margin_y());
    let background = canvas.and_then(|c| c.background.as_deref());

    // --- 4. Topology dispatch + layout, self-reviewed ------------------------
    let topology = rdg_dispatch::analyze(&compiled, &tokens);
    let mut decision = rdg_dispatch::dispatch(&topology, &tokens);
    if let Some(forced) = cli.layout.as_framework() {
        decision.framework = forced;
    }
    if compiled.diagram_type != "sequence" {
        report_dispatch(report, &decision, cli.layout == LayoutEngine::Auto);
    }

    let layout_config = LayoutConfig {
        rank_spacing,
        node_spacing,
        direction,
        margin_x,
        margin_y,
        group_gap_x,
        group_gap_y,
        tokens,
        ..LayoutConfig::default()
    };
    let max_passes = layout_config.tokens.max_review_passes;
    let reviewed = compute_reviewed_layout(&compiled, &layout_config, max_passes, &decision)
        .context("layout computation failed")?;
    let remaining_anomalies = reviewed.remaining_anomalies(&compiled, &layout_config.tokens);
    report_review(report, &reviewed, remaining_anomalies.len());
    for a in &remaining_anomalies {
        report.anomaly(&format!("{:?}", a.kind), &a.description);
    }
    if std::env::var("RDG_DEBUG_ANOMALIES").is_ok() {
        for a in &remaining_anomalies {
            eprintln!("  [{:?}] {}", a.kind, a.description);
        }
    }
    let layout_result = &reviewed.layout;
    let edge_plans = &reviewed.edge_plans;
    if std::env::var("RDG_DEBUG_SPACING").is_ok() {
        let m = rdg_render_core::review::spacing_metrics(&compiled, layout_result, edge_plans, &layout_config.tokens);
        let fmt = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{v:.1}"));
        eprintln!(
            "spacing: arrow={} node_gap={} port_pitch={} jogs={} crossings={} near_bend={} border_hugs={}  [arrow: {}; pitch: {}]",
            fmt(m.min_arrow_len),
            fmt(m.min_node_gap),
            fmt(m.min_port_pitch),
            m.micro_jogs,
            m.crossings,
            m.crossings_near_bend,
            m.border_hugs,
            m.min_arrow_edge.as_deref().unwrap_or("-"),
            m.min_pitch_face.as_deref().unwrap_or("-")
        );
    }

    // --- 5. Render ----------------------------------------------------------
    let output_path = Path::new(&cli.output);
    let is_svg = output_path.extension().and_then(|e| e.to_str()) == Some("svg");

    if is_svg {
        let drawio_xml = render_drawio(&compiled, layout_result, edge_plans, &theme, background, &layout_config.tokens)
            .context("draw.io XML rendering failed")?;

        let mut rendered_exact_drawio = false;
        if cli.svg_engine != "native" {
            if let Some(()) = try_export_svg_via_drawio_cli(&drawio_xml, output_path, theme.mode == rdg_render_core::theme::Mode::Dark) {
                rendered_exact_drawio = true;
            }
        }

        if !rendered_exact_drawio {
            if cli.svg_engine == "drawio" {
                anyhow::bail!(
                    "Exact draw.io export was requested (--svg-engine=drawio), but drawio CLI is not installed or failed"
                );
            }
            let svg_content = render_svg(&compiled, layout_result, edge_plans, &theme, background, &layout_config.tokens)
                .context("SVG rendering failed")?;
            fs::write(&cli.output, svg_content)
                .with_context(|| format!("failed to write output to '{}'", cli.output))?;
        } else {
            report.note("✓ Rendered exact draw.io SVG export via drawio CLI");
        }
    } else {
        let content = render_drawio(&compiled, layout_result, edge_plans, &theme, background, &layout_config.tokens)
            .context("draw.io XML rendering failed")?;
        fs::write(&cli.output, content)
            .with_context(|| format!("failed to write output to '{}'", cli.output))?;
    }

    report.written(&cli.output);

    Ok(if cli.strict && !remaining_anomalies.is_empty() { 1 } else { 0 })
}

/// The theme to render with. A theme *file* on the command line is used as written; a
/// theme *name* on the command line replaces the YAML's name but keeps its overrides
/// (so one diagram renders in any theme); otherwise the YAML's `theme:` decides.
fn resolve_theme(cli: Option<&str>, yaml: Option<&serde_yaml::Value>) -> Result<rdg_render_core::theme::Theme> {
    use rdg_render_core::theme::{DEFAULT_THEME, Theme};
    use serde_yaml::Value;
    if let Some(path) = cli.filter(|p| p.ends_with(".yaml") || p.ends_with(".yml")) {
        let src = fs::read_to_string(path).with_context(|| format!("failed to read theme file '{path}'"))?;
        return Theme::from_yaml(&src).with_context(|| format!("in theme file '{path}'"));
    }
    match (cli, yaml) {
        (Some(name), Some(Value::Mapping(m))) => {
            let mut m = m.clone();
            m.insert(Value::from("extends"), Value::from(name));
            Theme::from_value(Value::Mapping(m), DEFAULT_THEME)
        }
        (Some(name), _) => Theme::builtin(name),
        (None, Some(v)) => Theme::from_value(v.clone(), DEFAULT_THEME),
        (None, None) => Theme::builtin(DEFAULT_THEME),
    }
}

/// Loads [`DesignTokens`] from `path` if given, falling back to built-in defaults
/// otherwise. The file only needs to set the fields it wants to override — every
/// field is independently defaulted (`#[serde(default)]` on the struct), so e.g. a
/// file containing just `unit: 10.0` overrides the spacing scale while every other
/// token (font size, thresholds, algorithm budgets, ...) keeps its default.
fn load_design_tokens(path: Option<&str>) -> Result<DesignTokens> {
    let Some(path) = path else {
        return Ok(DesignTokens::default());
    };
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read design-tokens file '{path}'"))?;
    serde_yaml::from_str(&content)
        .with_context(|| format!("failed to parse design-tokens file '{path}' as YAML"))
}

/// Prints a one-line stderr summary of the topology dispatcher's choice: the framework,
/// its complexity estimate, and the deciding metric(s). Silent when the user explicitly
/// forced a framework via `--layout` — there's nothing to report in that case, the
/// choice was theirs, not the analyzer's.
fn report_dispatch(report: &mut Report, decision: &rdg_dispatch::AlgorithmDecision, was_auto: bool) {
    let framework = match decision.framework {
        rdg_dispatch::LayoutFramework::Sugiyama => "sugiyama",
        rdg_dispatch::LayoutFramework::ForceDirected => "force-directed",
        rdg_dispatch::LayoutFramework::FCose => "fcose",
    };
    report.layout(framework);
    if !was_auto {
        return;
    }
    report.note(&format!(
        "→  layout: {framework} ({}), routing: orthogonal search — {}",
        decision.complexity_estimate,
        decision.preprocessing_steps.join("; ")
    ));
}

/// Prints a concise stderr summary of the self-review retry loop: nothing at all for a
/// clean first pass, one line per retry otherwise, and a final resolved/remaining summary.
fn report_review(report: &Report, reviewed: &rdg_render_core::review::ReviewedLayout, final_count: usize) {
    if reviewed.passes.len() == 1 && reviewed.passes[0].anomaly_count == 0 {
        return;
    }
    for pass in &reviewed.passes {
        if pass.anomaly_count > 0 {
            report.note(&format!(
                "⚠  pass {}: {} anomal{} detected (rank_spacing={}, node_spacing={}){}",
                pass.attempt,
                pass.anomaly_count,
                if pass.anomaly_count == 1 { "y" } else { "ies" },
                pass.rank_spacing,
                pass.node_spacing,
                if pass.attempt < reviewed.passes.len() as u32 {
                    " — retrying with wider spacing"
                } else {
                    ""
                },
            ));
        }
    }
    // What matters is the layout `rdg` actually writes: the fewest-anomaly pass (widening
    // spacing helps a layered layout almost monotonically, but for the force-directed/fCoSE
    // engines a wider scale can just as easily make crossings worse, so the last pass isn't
    // necessarily the best) *after* the final polish pass, which can clear an anomaly the
    // review loop had given up on. `final_count` is that post-polish figure — the same one
    // `--strict` acts on — so this summary can never contradict the exit code.
    if final_count == 0 {
        if reviewed.passes.len() > 1 {
            report.note(&format!("✓  resolved after {} pass(es)", reviewed.passes.len()));
        } else {
            report.note("✓  resolved by the polish pass");
        }
    } else {
        report.note(&format!(
            "✗  {} anomal{} {} in the best attempt tried ({} pass(es)) — writing it anyway",
            final_count,
            if final_count == 1 { "y" } else { "ies" },
            if final_count == 1 { "remains" } else { "remain" },
            reviewed.passes.len(),
        ));
    }
}

/// Attempts to export exact draw.io SVG using the drawio desktop CLI if available in PATH or Applications.
fn try_export_svg_via_drawio_cli(drawio_xml: &str, output_path: &Path, dark: bool) -> Option<()> {
    let candidates = [
        "drawio",
        "/opt/homebrew/bin/drawio",
        "/Applications/draw.io.app/Contents/MacOS/draw.io",
    ];

    let mut temp_path = std::env::temp_dir();
    let unique_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(12345);
    temp_path.push(format!("rdg_export_{unique_id}.drawio"));

    fs::write(&temp_path, drawio_xml).ok()?;

    let theme_arg = if dark { "dark" } else { "light" };

    let mut exported = false;
    for cmd in candidates {
        let status = std::process::Command::new(cmd)
            .arg("-x")
            .arg("-f")
            .arg("svg")
            .arg("-e")
            .arg("--embed-svg-fonts")
            .arg("true")
            .arg("--theme")
            .arg(theme_arg)
            .arg("-o")
            .arg(output_path)
            .arg(&temp_path)
            .status();

        if let Ok(s) = status {
            if s.success() && output_path.exists() {
                exported = true;
                break;
            }
        }
    }

    let _ = fs::remove_file(&temp_path);

    if exported {
        strip_svg_unsupported_fallback(output_path);
        Some(())
    } else {
        None
    }
}

/// The draw.io CLI always appends a `<switch>` fallback to its SVG export — a link
/// reading "Text is not SVG - cannot display", meant for viewers that don't support
/// the `requiredFeatures` primary branch. Real browsers (this output's actual target
/// per rdg's own docs: "embed in docs, Markdown, HTML") satisfy that feature check and
/// never show it, but several common non-browser SVG consumers — librsvg-based tools
/// (`rsvg-convert`, GNOME's thumbnailer) among them — evaluate it differently and
/// render the fallback text as visible on-canvas content instead, confirmed directly
/// while visually reviewing rendered sample diagrams. rdg's own content never needs
/// that fallback (it's plain rects/paths/text, not the HTML-label case the check
/// exists for), so it's dead weight at best and a rendering artifact at worst —
/// stripped here rather than left for every downstream viewer to handle differently.
/// Best-effort: a failure to re-read/rewrite the file just leaves draw.io's own output
/// as-is, no different from before this post-processing step existed.
fn strip_svg_unsupported_fallback(output_path: &Path) {
    let Ok(svg) = fs::read_to_string(output_path) else {
        return;
    };
    const MARKER: &str = "<switch><g requiredFeatures=";
    const CLOSE: &str = "</switch>";
    let Some(start) = svg.find(MARKER) else {
        return;
    };
    let Some(close_rel) = svg[start..].find(CLOSE) else {
        return;
    };
    let end = start + close_rel + CLOSE.len();
    let mut cleaned = String::with_capacity(svg.len() - (end - start));
    cleaned.push_str(&svg[..start]);
    cleaned.push_str(&svg[end..]);
    let _ = fs::write(output_path, cleaned);
}
