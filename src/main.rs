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
    /// Path to the YAML diagram payload. Omit (or pass -) to read from stdin.
    ///
    /// The YAML must follow the rdg schema:
    ///   diagram_type, theme (optional), nodes[], edges[]
    /// See `rdg --help` for the full schema reference.
    #[arg(short, long, value_name = "FILE")]
    input: Option<String>,

    /// Output file path. Extension selects the format:
    ///   .drawio → uncompressed mxfile XML (for draw.io / diagrams.net)
    ///   .svg    → Scalable Vector Graphics
    #[arg(short, long, default_value = "output.drawio", value_name = "FILE")]
    output: String,

    /// Spatial layout algorithm.
    ///
    /// sugiyama  — hierarchical top-to-bottom DAG layout (default, recommended)
    /// orthogonal — right-angled grid routing (reserved, falls back to sugiyama)
    /// organic    — force-directed placement   (reserved, falls back to sugiyama)
    #[arg(short, long, value_enum, default_value_t = LayoutEngine::Sugiyama)]
    layout: LayoutEngine,

    /// Visual theme for node colours and styles.
    ///
    /// Supported: standard (default), aws, azure.
    /// Can also be set per-diagram via the 'theme' key in the YAML payload.
    #[arg(short, long, default_value = "standard", value_name = "THEME")]
    theme: String,

    /// Vertical gap in pixels between successive ranks (layers) of nodes.
    /// Increase for more breathing room between layers. Default: 36.
    #[arg(long, default_value_t = 36)]
    rank_spacing: u32,

    /// Horizontal gap in pixels between nodes on the same rank.
    /// Increase to prevent label overlap. Default: 20.
    #[arg(long, default_value_t = 20)]
    node_spacing: u32,

    /// Flow direction of the diagram.
    ///
    /// tb — top-to-bottom (default, recommended for DAGs & hierarchies)
    /// lr — left-to-right (recommended for pipelines & sequence flows)
    #[arg(long, value_enum, default_value_t = CliDirection::Tb)]
    direction: CliDirection,
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
        .context("YAML payload does not match the rdg schema — run `rdg --help` for schema reference")?;

    // YAML 'theme' key overrides CLI --theme flag.
    let theme = payload.theme.as_deref().unwrap_or(&cli.theme);

    // Flow direction priority: YAML payload > CLI flag
    let direction = match payload.direction.as_deref() {
        Some("lr") | Some("LR") | Some("left_to_right") => rdg::layout::LayoutDirection::LeftToRight,
        Some("tb") | Some("TB") | Some("top_to_bottom") => rdg::layout::LayoutDirection::TopToBottom,
        _ => match cli.direction {
            CliDirection::Tb => rdg::layout::LayoutDirection::TopToBottom,
            CliDirection::Lr => rdg::layout::LayoutDirection::LeftToRight,
        },
    };

    // --- 3. Build petgraph --------------------------------------------------
    let compiled = build_graph(&payload)?;
    if compiled.had_cycles {
        eprintln!(
            "⚠  Cycles detected in input graph — automatically broken via Feedback Arc Set."
        );
    }

    // --- 4. Layout ----------------------------------------------------------
    let layout_config = LayoutConfig {
        rank_spacing: cli.rank_spacing,
        node_spacing: cli.node_spacing,
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
