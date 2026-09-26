//! `rdg` (Render Diagram) — binary entry point.
//!
//! Parses CLI arguments, reads the YAML payload, runs the pipeline, and writes
//! the output file. All heavy lifting is in the `rdg` library crate.

use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};

use rdg::{
    graph::build_graph,
    layout::{compute_layout, LayoutConfig},
    render::{render_drawio, render_svg},
    schema::DiagramPayload,
};

// ---------------------------------------------------------------------------
// Long help text — LLM-optimised
// ---------------------------------------------------------------------------

const LONG_ABOUT: &str = "\
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  rdg  ·  Render Diagram  ·  v1.0
  Deterministic graph layout compiler for LLM-generated semantic diagrams.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

ROLE OF THIS TOOL (FOR LLM AGENTS)
───────────────────────────────────
You (the LLM) are responsible for semantic reasoning: deciding which nodes
exist, what they represent, and how they connect. rdg is responsible for
spatial reasoning: computing pixel-perfect, non-overlapping coordinates and
serialising a strictly valid output file.

Do NOT attempt to compute x/y coordinates, widths, or geometry yourself.
Do NOT write raw draw.io XML or SVG. Instead:
  1. Emit the token-minimal YAML schema described below.
  2. Pipe it to rdg or save it to a file and pass with --input.
  3. rdg handles all layout, styling, and serialisation.

This separation prevents coordinate hallucinations and XML syntax errors.

─────────────────────────────────────────────────────────────────────────────
INPUT SCHEMA (YAML) — emit exactly this structure
─────────────────────────────────────────────────────────────────────────────

  diagram_type: flowchart          # required — logical category (string)
  theme: standard                  # optional — standard (white-card) or dark
  groups:                          # optional — visual swimlane containers
    - id: g1
      label: \"Core Cluster\"        # container title
      color: \"#0284c7\"             # optional accent hex color
      nodes: [n1, n2]              # node ids enclosed in container
  nodes:
    - id: n1                       # required — unique, short, no spaces
      label: \"API Gateway\\nv2\"     # required — title, newline adds subtitle
      type: proxy                  # optional — semantic type (see NODE TYPES)
      metadata: \"tooltip text\"     # optional — free annotation / tooltip
  edges:
    - from: n1                     # required — source node id
      to:   n2                     # required — target node id
      label: \"edge label\"          # optional — text along the connector
      edge_style: async            # optional: flow | async | error | data | bidirectional

Rules:
  • ids must be unique across all nodes and groups.
  • Edge from/to values must reference existing node ids.
  • Cycles are allowed — rdg breaks them automatically via ELS Feedback Arc Set.
  • Unknown node types fall back to the default rounded-card style.
  • The 'theme' key in YAML takes precedence over --theme flag.

─────────────────────────────────────────────────────────────────────────────
NODE TYPES (WHITE-CARD DESIGN SYSTEM)
─────────────────────────────────────────────────────────────────────────────

  All nodes render as clean elevated white cards with semantic border accents:

  proxy / gateway / api      → Indigo accent card  (API gateways, load balancers)
  server / service / backend → Emerald accent card (microservices, backends)
  database / db / storage    → Sky 3D cylinder     (databases, object stores)
  queue / broker / bus       → Amber queue pill    (Kafka, RabbitMQ, SQS)
  cache / redis / memcache   → Rose diamond        (Redis, Memcached, CDN edge)
  function / lambda / faas   → Orange Lambda card  (serverless handlers)
  client / user / browser    → Slate person icon   (end-users, web clients)
  decision / condition       → Purple diamond      (branching logic)
  (anything else)            → Neutral slate card  (generic component)

─────────────────────────────────────────────────────────────────────────────
INLINE TYPOGRAPHY & MARKDOWN FORMATTING
─────────────────────────────────────────────────────────────────────────────

  Labels in nodes, edges, and groups support rich inline typography:

  `code`             → Monospace font (JetBrains Mono / Menlo) for types, APIs, filenames
  **bold**           → Bold weight for emphasis
  *italic*           → Italic styling for status or notes
  __underline__      → Underline styling
  ~~strike~~         → Strikethrough for deprecated/removed components
  ~sub~              → Subscript (e.g. H~2~O)
  ^super^            → Superscript (e.g. O(N^2^))
  $math$ or \\(math\\) → LaTeX math (e.g. $R \\times C$, $\\alpha = 0.5$)
                       Rendered natively via MathJax in draw.io (math=\"1\")
                       and crisp Unicode mathematical glyphs in SVG.

  Multi-line Titles & Subtitles:
  • Plain and code lines are formatted as prominent BOLD TITLES.
    E.g. `petgraph::`\\n`StableDiGraph` → both lines bold monospace titles.
  • Parenthesized lines (subtitle) or bracketed [detail] or {fields}
    are automatically formatted as muted 10px SUBTITLES.
    E.g. `clap::Cli`\\n(CLI Arguments) → bold title on top, muted subtitle below.

─────────────────────────────────────────────────────────────────────────────
EDGE STYLES (edge_style)
─────────────────────────────────────────────────────────────────────────────

  flow (default)   → Solid slate line, filled block arrow
  async            → Dashed amber line, open arrow (events, queues)
  error / fallback → Dashed red line, hollow arrow (dead-letter, circuit breaker)
  data / stream    → 2px Indigo line, filled block arrow (data replication)
  bidirectional    → Solid slate line, dual arrows (WebSocket, full-duplex)

─────────────────────────────────────────────────────────────────────────────
OUTPUT FORMATS — extension controls format
─────────────────────────────────────────────────────────────────────────────

  .drawio   Uncompressed mxfile XML — open in draw.io desktop / diagrams.net
  .svg      Scalable Vector Graphics — embed in docs, Markdown, HTML

─────────────────────────────────────────────────────────────────────────────
LAYOUT ALGORITHMS
─────────────────────────────────────────────────────────────────────────────

  sugiyama  (default) Hierarchical top-to-bottom DAG layout using the
                      Sugiyama framework. Best for: flowcharts, pipelines,
                      data-flow diagrams, system architectures, CI/CD.
                      Cycles are auto-broken (reversed edges are marked).

  orthogonal          Right-angled grid routing. Best for: UML class diagrams,
                      ER diagrams, network topology. (Falls back to sugiyama
                      in current release.)

  organic             Force-directed layout. Best for: dense, unstructured
                      networks, social graphs, dependency webs. (Falls back
                      to sugiyama in current release.)

─────────────────────────────────────────────────────────────────────────────
QUICKSTART EXAMPLES
─────────────────────────────────────────────────────────────────────────────

  # Minimal: read from stdin, write draw.io file
  rdg --output architecture.drawio << 'EOF'
  diagram_type: flowchart
  nodes:
    - id: n1
      label: \"Client\"
      type: client
    - id: n2
      label: \"API Gateway\"
      type: proxy
    - id: n3
      label: \"Database\"
      type: database
  edges:
    - from: n1
      to: n2
      label: \"HTTPS\"
    - from: n2
      to: n3
      label: \"SQL\"
  EOF

  # From a file, output SVG, custom spacing
  rdg --input diagram.yaml --output diagram.svg --rank-spacing 80 --node-spacing 50

  # Explicit layout and theme
  rdg -i graph.yaml -o out.drawio --layout sugiyama --theme aws

─────────────────────────────────────────────────────────────────────────────
LLM AGENT WORKFLOW (RECOMMENDED PROMPT PATTERN)
─────────────────────────────────────────────────────────────────────────────

  When asked to generate a diagram, follow these steps:

  Step 1 — Reason about nodes and relationships in natural language.
  Step 2 — Emit ONLY the YAML block (no extra prose, no XML, no SVG).
           Use short ids (n1, n2, …) to minimise token count.
  Step 3 — Pipe or save the YAML, then invoke:
             rdg --input diagram.yaml --output diagram.drawio
  Step 4 — Report the output file path to the user.

  DO NOT hallucinate pixel coordinates.
  DO NOT write mxGeometry tags.
  DO NOT wrap the YAML in JSON or any other envelope.
  DO NOT invent node types not listed above (unknown types render safely).

─────────────────────────────────────────────────────────────────────────────
NOTES FOR AGENTIC PIPELINES
─────────────────────────────────────────────────────────────────────────────

  • rdg exits 0 on success, non-zero on error.
  • All diagnostic messages go to stderr; the output file is pure content.
  • The tool is deterministic: same YAML input always produces the same output.
  • Cycle warnings are printed to stderr and do not affect exit code.
  • You can chain: llm-generate | rdg -o out.drawio && open out.drawio
";

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// rdg (Render Diagram) — token-efficient LLM diagram compiler.
///
/// Accepts a flat YAML semantic graph, runs deterministic layout algorithms,
/// and emits a pixel-perfect draw.io XML or SVG file.
#[derive(Parser, Debug)]
#[command(
    name = "rdg",
    version = "1.0",
    author = "rdg contributors",
    about = "Render Diagram: compile LLM semantic graphs into draw.io XML or SVG.",
    long_about = LONG_ABOUT,
    after_help = "Tip: run `rdg --help` to see the full LLM usage guide including YAML schema and node types."
)]
struct Cli {
    /// Path to input YAML file (reads stdin if omitted or '-').
    #[arg(
        short,
        long,
        value_name = "FILE",
        help = "Path to input YAML file (reads stdin if omitted or '-')",
        long_help = "Path to the input YAML diagram payload.\n\
                     \n\
                     If omitted or set to '-', rdg reads from standard input (stdin),\n\
                     enabling clean Unix piping in scripts and LLM generation pipelines:\n\
                       cat diagram.yaml | rdg -o diagram.svg\n\
                       llm-agent-command | rdg -o arch.drawio\n\
                     \n\
                     The YAML payload must conform to the rdg schema (nodes, edges, groups).\n\
                     Run `rdg --example` to see a full reference template, or `rdg --schema`\n\
                     for the JSON Schema definition."
    )]
    input: Option<String>,

    /// Output diagram file path (.drawio or .svg).
    #[arg(
        short,
        long,
        default_value = "output.drawio",
        value_name = "FILE",
        help = "Output diagram file path (.drawio or .svg)",
        long_help = "Path where the finished diagram will be written.\n\
                     \n\
                     The file extension selects the output serialization format:\n\
                     \n\
                       .drawio  Uncompressed XML in the standard mxfile format.\n\
                                • 100% native draw.io / diagrams.net compatibility\n\
                                • Full editability: drag, resize, and modify in draw.io\n\
                                • Container grouping: moving a group moves all member nodes\n\
                                • MathJax support (math=\"1\"): renders LaTeX equations\n\
                                • Rich HTML labels with code monospace and bold titles\n\
                                • Rounded orthogonal edges with line jump arcs (jumpStyle=arc)\n\
                     \n\
                       .svg     Standalone Scalable Vector Graphics.\n\
                                • Crisp vector rendering for web, Markdown, and docs\n\
                                • Modern elevated card styling with drop shadows\n\
                                • Orthogonal rounded fillet connector paths (no line collisions)\n\
                                • Styled <tspan> elements with Unicode mathematical glyphs\n\
                                • Embed directly in GitHub READMEs, Notion, and HTML"
    )]
    output: String,

    /// Spatial layout algorithm (sugiyama, orthogonal, organic).
    #[arg(
        short,
        long,
        value_enum,
        default_value_t = LayoutEngine::Sugiyama,
        help = "Spatial layout algorithm (sugiyama, orthogonal, organic)",
        long_help = "Spatial layout algorithm used to compute coordinates.\n\
                     \n\
                     Available engines:\n\
                     \n\
                       sugiyama   (Default, Recommended)\n\
                                  Layered hierarchical DAG layout implementing:\n\
                                  1. Cycle breaking: Greedy Feedback Arc Set (FAS)\n\
                                     reverses back-edges to guarantee a valid DAG.\n\
                                  2. Layer assignment: Longest-path topological ranking.\n\
                                  3. Crossing minimization: 3-pass alternating barycentric\n\
                                     median heuristics with adjacent transpositions.\n\
                                  4. 2D Compound Quotient Layout: Resolves inter-group\n\
                                     dependencies using grid search to optimize canvas\n\
                                     aspect ratio close to 1.0 (squarish canvas).\n\
                                  5. Compact whitespace normalization: Eliminates dead\n\
                                     canvas margins and centers ranks.\n\
                     \n\
                       orthogonal Right-angled orthogonal grid routing. Ideal for UML\n\
                                  class diagrams and ER schemas. (Reserved for v2.0;\n\
                                  currently falls back to Sugiyama.)\n\
                     \n\
                       organic    Force-directed spring electrical embedder for unstructured\n\
                                  graphs and social networks. (Reserved for v2.0;\n\
                                  currently falls back to Sugiyama.)"
    )]
    layout: LayoutEngine,

    /// Visual theme (standard, dark).
    #[arg(
        short,
        long,
        default_value = "standard",
        value_name = "THEME",
        help = "Visual theme (standard, dark)",
        long_help = "Visual color palette and card styling.\n\
                     \n\
                     Themes:\n\
                     \n\
                       standard  (Default)\n\
                                 Modern elevated white-card design system on a clean\n\
                                 light slate background (#f8fafc). Each node renders\n\
                                 as a #ffffff card with a subtle drop shadow, 8px\n\
                                 rounded corners, and semantic border color accents.\n\
                     \n\
                       dark      High-contrast dark mode on a deep Slate-900 canvas\n\
                                 (#0f172a). Nodes render with Slate-800 card bodies\n\
                                 (#1e293b), Slate-600 borders (#475569), and bright\n\
                                 typography (#f1f5f9).\n\
                     \n\
                     Note: Can also be set inside the YAML payload via `theme: dark`.\n\
                     YAML setting overrides this CLI flag."
    )]
    theme: String,

    /// Vertical gap between successive ranks in pixels (default: 44).
    #[arg(
        long,
        default_value_t = 44,
        value_name = "PIXELS",
        help = "Vertical gap between successive ranks in pixels (default: 44)",
        long_help = "Gap in pixels between successive layers (ranks) of nodes.\n\
                     \n\
                     • In top-to-bottom (tb) mode, this controls vertical distance between ranks.\n\
                     • In left-to-right (lr) mode, this controls horizontal distance between columns.\n\
                     \n\
                     Guidelines:\n\
                       28–36 px  Compact layout (dashboards, dense architectures)\n\
                       44 px     Default balanced layout (expert hand-drawn feel)\n\
                       60–80 px  Roomy layout (multi-line edge labels, long routing spans)\n\
                     \n\
                     Note: Can also be specified in YAML via `rank_spacing: 60`."
    )]
    rank_spacing: u32,

    /// Horizontal gap between sibling nodes on the same rank (default: 28).
    #[arg(
        long,
        default_value_t = 28,
        value_name = "PIXELS",
        help = "Horizontal gap between sibling nodes on the same rank (default: 28)",
        long_help = "Clearance in pixels between sibling nodes sharing the same rank.\n\
                     \n\
                     Guidelines:\n\
                       18–24 px  Tight clustering for compact diagrams\n\
                       28 px     Default balanced clearance\n\
                       40–50 px  Wide spacing to prevent edge routing congestion\n\
                     \n\
                     Note: Can also be specified in YAML via `node_spacing: 36`."
    )]
    node_spacing: u32,

    /// Diagram flow direction (tb, lr).
    #[arg(
        long,
        value_enum,
        default_value_t = CliDirection::Tb,
        help = "Diagram flow direction (tb, lr)",
        long_help = "Overall flow direction of the diagram hierarchy.\n\
                     \n\
                     Options:\n\
                     \n\
                       tb   Top-to-Bottom (Default)\n\
                            Hierarchical downward flow. Best for:\n\
                            • Microservice architectures and cloud topology\n\
                            • Call graphs and dependency trees\n\
                            • Decision trees and state transition diagrams\n\
                     \n\
                       lr   Left-to-Right\n\
                            Horizontal sequential flow. Best for:\n\
                            • CI/CD and build/release pipelines\n\
                            • Event streaming architectures (Kafka / Flink / ETL)\n\
                            • Request/response lifecycles and sequence pipelines\n\
                     \n\
                     Note: Can also be specified inside YAML via `direction: lr` or `direction: tb`.\n\
                     YAML setting overrides this CLI flag."
    )]
    direction: CliDirection,

    /// Print a complete reference YAML template to stdout and exit.
    #[arg(
        long,
        help = "Print a complete reference YAML template to stdout and exit",
        long_help = "Print a production-ready, fully commented YAML diagram template to stdout.\n\
                     \n\
                     Demonstrates every feature:\n\
                       • Top-level diagram metadata (title, description, direction, theme)\n\
                       • Visual group containers (swimlanes)\n\
                       • All semantic node types (proxy, database, queue, server, decision, etc.)\n\
                       • All edge styles (flow, async, error, data, bidirectional)\n\
                       • Inline typography (monospace `code`, **bold**, *italic*, $LaTeX math$)\n\
                     \n\
                     Usage for LLMs:\n\
                       rdg --example > template.yaml"
    )]
    example: bool,

    /// Print JSON Schema specification for the YAML payload and exit.
    #[arg(
        long,
        help = "Print JSON Schema specification for the YAML payload and exit",
        long_help = "Print the JSON Schema (draft-07) for the YAML diagram payload to stdout.\n\
                     \n\
                     Enables:\n\
                       • Schema-guided decoding in LLM tool calling (structured output)\n\
                       • Automated validation of generated YAML files in CI/CD\n\
                       • IDE autocompletion and hover documentation in VS Code / IntelliJ\n\
                     \n\
                     Usage:\n\
                       rdg --schema > schema.json"
    )]
    schema: bool,
}

/// Diagram flow direction.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum CliDirection {
    /// Top-to-bottom flow.
    Tb,
    /// Left-to-right flow.
    Lr,
}

/// Layout algorithm to use for spatial positioning.
#[derive(ValueEnum, Clone, Debug)]
enum LayoutEngine {
    /// Hierarchical Sugiyama framework for DAGs — top-to-bottom flow.
    /// Automatically breaks cycles using a greedy Feedback Arc Set.
    Sugiyama,
    /// Orthogonal right-angled grid routing for UML/ER diagrams.
    /// (Reserved — currently falls back to Sugiyama.)
    Orthogonal,
    /// Force-directed layout for dense unstructured networks.
    /// (Reserved — currently falls back to Sugiyama.)
    Organic,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();

    // --- Special flags: --example and --schema -------------------------------
    if cli.example {
        print!("{}", DiagramPayload::example_yaml());
        return Ok(());
    }
    if cli.schema {
        print!("{}", DiagramPayload::json_schema());
        return Ok(());
    }

    // --- 1. Read YAML input -------------------------------------------------
    let yaml = match cli.input.as_deref() {
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
    let payload = DiagramPayload::from_yaml(&yaml)
        .context("YAML payload does not match the rdg schema — run `rdg --help` or `rdg --example`")?;

    // Priority: YAML payload > CLI flag
    let theme = payload.theme.as_deref().unwrap_or(&cli.theme);

    let direction = payload.resolved_direction().unwrap_or(match cli.direction {
        CliDirection::Tb => rdg::layout::LayoutDirection::TopToBottom,
        CliDirection::Lr => rdg::layout::LayoutDirection::LeftToRight,
    });

    let rank_spacing = payload.rank_spacing.unwrap_or(cli.rank_spacing);
    let node_spacing = payload.node_spacing.unwrap_or(cli.node_spacing);

    // --- 3. Build petgraph --------------------------------------------------
    let compiled = build_graph(&payload)?;
    if compiled.had_cycles {
        eprintln!(
            "⚠  Cycles detected in input graph — automatically broken via Feedback Arc Set."
        );
    }

    // --- 4. Layout ----------------------------------------------------------
    let layout_config = LayoutConfig {
        rank_spacing,
        node_spacing,
        direction,
        ..LayoutConfig::default()
    };
    let layout_result = compute_layout(&compiled, &layout_config)?;

    // --- 5. Render ----------------------------------------------------------
    let output_path = Path::new(&cli.output);
    let content = match output_path.extension().and_then(|e| e.to_str()) {
        Some("svg") => render_svg(&compiled, &layout_result, theme)
            .context("SVG rendering failed")?,
        _ => render_drawio(&compiled, &layout_result, theme)
            .context("draw.io XML rendering failed")?,
    };

    // --- 6. Write output file -----------------------------------------------
    fs::write(&cli.output, content)
        .with_context(|| format!("failed to write output to '{}'", cli.output))?;

    eprintln!("✓ Diagram written to {}", cli.output);
    Ok(())
}
