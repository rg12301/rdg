//! `rdg` (Render Diagram) — binary entry point.
//!
//! Parses CLI arguments, reads the YAML payload, runs the pipeline, and writes
//! the output file. All heavy lifting is in the `rdg-schema`/`rdg-graph`/`rdg-layout`/
//! `rdg-render-drawio`/`rdg-render-svg` library crates.

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

  diagram_type: architecture       # architecture | flowchart | sequence | erd | class | state
  theme: standard                  # optional — standard (white-card) or dark
  direction: tb                    # optional — tb (top-to-bottom) or lr (left-to-right)
  groups:                          # optional — visual swimlane containers
    - id: g1
      label: \"Core Cluster\"        # container title
      color: \"#0284c7\"             # optional accent hex color
      nodes: [n1, n2]              # node ids enclosed in container
  nodes:
    - id: n1                       # required — unique, short, no spaces
      label: \"Order Service\"       # required — component title
      type: service                # optional — semantic type (see NODE TYPES)
      language: rust               # optional — flat vector logo: rust, go, python, ts, java...
      technology: \"Axum 0.7\"       # optional — tech stack badge
      metadata: \"tooltip text\"     # optional — free annotation / tooltip
    - id: n2
      label: \"orders\"              # required — table / db entity
      type: database               # database or table renders 3D cylinder
      db_type: postgres            # optional — postgres, mysql, redis, mongo, kafka...
      fields:                      # optional — column or member rows
        - \"id: UUID [PK]\"
        - \"user_id: UUID [FK]\"
        - \"amount: DECIMAL\"
  edges:
    - from: n1                     # required — source node id
      to:   n2                     # required — target node id
      label: \"SQL INSERT\"          # optional — text along connector / protocol
      edge_style: one_to_many      # optional: flow | async | sync | reply | one_to_many | inheritance...

Rules:
  • ids must be unique across all nodes and groups.
  • Edge from/to values must reference existing node ids.
  • Cycles are allowed — rdg breaks them automatically via ELS Feedback Arc Set.
  • Unknown node types fall back to the default rounded-card style.
  • The 'theme' key in YAML takes precedence over --theme flag.

─────────────────────────────────────────────────────────────────────────────
DIAGRAM TYPES & ARCHITECTURE STANDARDS
─────────────────────────────────────────────────────────────────────────────

  architecture (HLD / LLD / C4)
    • Level 2 (Container) & Level 3 (Component) architecture diagrams.
    • Services display vector language badges and technology stack tags.
    • Databases render 3D cylinders with database engine badges.
    • Edge labels should name explicit communication protocols (e.g. gRPC, HTTPS, SQL).

  flowchart / graph
    • General computational pipelines, logic flows, state transitions.

  sequence
    • Participant lifelines at top; time flows strictly downward.
    • Solid lines with filled arrows for sync requests; dashed lines with open
      arrows for replies (edge_style: reply) and async events (edge_style: async).

  erd (Entity Relationship Diagram)
    • Tables rendered as cylinders or structured entity cards with [PK] and [FK] fields.
    • Connectors feature standard Crow's Foot notation:
      one_to_many (1:N), many_to_many (M:N), one_to_one (1:1), zero_to_many (0:N).

  class (UML Class Diagram)
    • Structured compartment cards with member fields and methods.
    • Stereotype badges: <<interface>>, <<abstract>>.
    • Connectors: inheritance (hollow triangle), realization (dashed triangle),
      composition (filled diamond), aggregation (hollow diamond), dependency (dashed open).

─────────────────────────────────────────────────────────────────────────────
NODE TYPES (WHITE-CARD DESIGN SYSTEM)
─────────────────────────────────────────────────────────────────────────────

  All nodes render as clean elevated white cards with semantic border accents:

  proxy / gateway / api      → Indigo accent card  (API gateways, load balancers)
  server / service / backend → Emerald accent card (microservices, backends)
  database / db / storage    → Sky 3D cylinder     (databases, persistent stores)
  table / entity / record    → Sky 3D cylinder     (database tables, entities)
  queue / broker / bus       → Amber queue pill    (Kafka, RabbitMQ, SQS)
  cache / redis / memcache   → Rose diamond        (Redis, Memcached, CDN edge)
  function / lambda / faas   → Orange Lambda card  (serverless handlers)
  client / user / browser    → Slate person icon   (end-users, web clients)
  decision / condition       → Purple diamond      (branching logic)
  class / abstract_class     → UML structured card (classes with methods/fields)
  interface                  → UML interface card  (<<interface>> stereotype)
  start / start_state        → Solid filled circle (entry point)
  end / end_state            → Bullseye circle     (terminal state)
  (anything else)            → Neutral slate card  (generic component)

─────────────────────────────────────────────────────────────────────────────
SUPPORTED FLAT VECTOR ICONS
─────────────────────────────────────────────────────────────────────────────

  Languages:
    rust, go, python, typescript, javascript, java, kotlin, cpp, csharp,
    ruby, swift, node

  Databases & Message Stores:
    postgres, mysql, redis, mongodb, dynamodb, kafka, cassandra, sqlite,
    elasticsearch, database, table, user

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

─────────────────────────────────────────────────────────────────────────────
EDGE STYLES (edge_style)
─────────────────────────────────────────────────────────────────────────────

  General:
    flow (default)   → Solid slate line, filled block arrow
    async            → Dashed amber line, open arrow (events, queues)
    error / fallback → Dashed red line, hollow arrow (dead-letter, circuit breaker)
    data / stream    → 2px Indigo line, filled block arrow (data replication)
    bidirectional    → Solid slate line, dual arrows (WebSocket, full-duplex)
    sync / call      → Synchronous call arrow
    reply / return   → Dashed return message arrow

  Entity Relationship (ER):
    one_to_many      → Crow's foot one-to-many (1:N)
    many_to_many     → Crow's foot many-to-many (M:N)
    one_to_one       → Crow's foot one-to-one (1:1)
    zero_to_many     → Crow's foot zero-to-many (0:N)

  UML Class Relationships:
    inheritance      → Solid line with hollow triangle arrowhead
    realization      → Dashed line with hollow triangle arrowhead
    composition      → Solid line with filled diamond start marker
    aggregation      → Solid line with hollow diamond start marker
    dependency       → Dashed line with open arrowhead

─────────────────────────────────────────────────────────────────────────────
OUTPUT FORMATS — extension controls format
─────────────────────────────────────────────────────────────────────────────

  .drawio   Uncompressed mxfile XML — open in draw.io desktop / diagrams.net
  .svg      Scalable Vector Graphics — embed in docs, Markdown, HTML

─────────────────────────────────────────────────────────────────────────────
LAYOUT ALGORITHMS
─────────────────────────────────────────────────────────────────────────────

  auto      (default) A topology analyzer inspects the input graph (cycles,
                      edge density, compound/nested group structure, connected
                      components) and dispatches to whichever engine below
                      fits — a one-line summary is printed to stderr.

  sugiyama            Hierarchical top-to-bottom DAG layout. Best for:
                      flowcharts, pipelines, data-flow diagrams, system
                      architectures, CI/CD. Cycles are auto-broken (reversed
                      edges are marked).

  force               Barnes-Hut force-directed layout. Best for: dense or
                      very large graphs, social graphs, dependency webs.

  fcose               Compound spring embedder. Best for: diagrams with
                      visual groups or disconnected components.

  Edge routing (a fixed corridor heuristic vs. a visibility-graph A* search)
  is chosen independently of --layout, based on obstacle density.

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

    /// Layout framework: auto (topology-dispatched), sugiyama, force, or fcose.
    #[arg(
        short,
        long,
        value_enum,
        default_value_t = LayoutEngine::Auto,
        help = "Layout framework (auto, sugiyama, force, fcose)",
        long_help = "Which layout framework computes node coordinates.\n\
                     \n\
                     Available engines:\n\
                     \n\
                       auto      (Default, Recommended)\n\
                                 Runs a topology analyzer over the input graph (cyclicity,\n\
                                 edge density, compound/nested group structure, connected\n\
                                 components) and dispatches to whichever of the three engines\n\
                                 below fits that shape best. A one-line summary of the choice\n\
                                 and why is printed to stderr. This is what most diagrams\n\
                                 should use — the other three values are for forcing a\n\
                                 specific engine regardless of what the analyzer would pick.\n\
                     \n\
                       sugiyama  Layered hierarchical DAG layout implementing:\n\
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
                                 Best for: dependency DAGs and chronological process flows.\n\
                     \n\
                       force     Barnes-Hut force-directed layout (O(N log N) repulsion).\n\
                                 Best for: dense or very large graphs that don't have a clean\n\
                                 hierarchical shape.\n\
                     \n\
                       fcose     Compound spring embedder: spectral draft layout + physical\n\
                                 relaxation, with grouped nodes pulled toward each other.\n\
                                 Best for: diagrams with visual groups or disconnected\n\
                                 components.\n\
                     \n\
                     Edge routing is chosen independently of this flag, based on how\n\
                     obstacle-dense the diagram is: a cheap corridor heuristic for small/\n\
                     sparse diagrams, or a visibility-graph A* search (penalizing bends and\n\
                     obstacle proximity) once there are enough nodes that global routing\n\
                     awareness actually pays for itself."
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

    /// Vertical gap between successive ranks in pixels (default: auto, proportional
    /// to node size — see `--design-config`'s `rank_spacing_fraction`).
    #[arg(
        long,
        value_name = "PIXELS",
        help = "Vertical gap between successive ranks in pixels (default: auto, scales with node size)",
        long_help = "Gap in pixels between successive layers (ranks) of nodes.\n\
                     \n\
                     • In top-to-bottom (tb) mode, this controls vertical distance between ranks.\n\
                     • In left-to-right (lr) mode, this controls horizontal distance between columns.\n\
                     \n\
                     Left unset (the default), this is computed at runtime as this diagram's own\n\
                     average node size times `rank_spacing_fraction` (a `--design-config` token,\n\
                     default 0.7) — bigger boxes automatically get proportionally more room, and\n\
                     retuning overall compactness is one fraction rather than a pixel count tied to\n\
                     whatever node size this particular diagram happens to have.\n\
                     \n\
                     Passing this flag pins an exact, non-proportional pixel value instead.\n\
                     \n\
                     Note: Can also be specified in YAML via `rank_spacing: 60`."
    )]
    rank_spacing: Option<u32>,

    /// Horizontal gap between sibling nodes on the same rank (default: auto,
    /// proportional to node size — see `--design-config`'s `node_spacing_fraction`).
    #[arg(
        long,
        value_name = "PIXELS",
        help = "Horizontal gap between sibling nodes on the same rank (default: auto, scales with node size)",
        long_help = "Clearance in pixels between sibling nodes sharing the same rank.\n\
                     \n\
                     Left unset (the default), this is computed at runtime as this diagram's own\n\
                     average node size times `node_spacing_fraction` (a `--design-config` token,\n\
                     default 0.4) — the same proportional reasoning as `--rank-spacing`.\n\
                     \n\
                     Passing this flag pins an exact, non-proportional pixel value instead.\n\
                     \n\
                     Note: Can also be specified in YAML via `node_spacing: 36`."
    )]
    node_spacing: Option<u32>,

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

    /// SVG export engine when writing .svg files: 'auto' (use draw.io CLI if available, else native),
    /// 'drawio' (require exact draw.io export), or 'native' (pure-Rust SVG renderer).
    #[arg(
        long,
        default_value = "auto",
        help = "SVG export engine: 'auto' (draw.io CLI if available, else native), 'drawio', or 'native'"
    )]
    svg_engine: String,

    /// Exit non-zero if geometric anomalies (overlaps, edges cutting through nodes, an
    /// explicit size too small for its content, an unreadable aspect ratio) remain after
    /// rdg's self-review retry budget is exhausted. Off by default: rdg always writes its
    /// best attempt regardless, this flag only controls whether that's treated as failure.
    #[arg(
        long,
        help = "Exit non-zero if self-review anomalies remain after the retry budget",
        long_help = "By default rdg auto-retries layout (wider spacing) up to a few times \
                     when it detects geometric anomalies — overlapping nodes, an edge cutting \
                     through an unrelated node, an explicit width/height too small for its \
                     content, or a canvas aspect ratio too lopsided to read as a diagram — \
                     and always writes its best (fewest-anomaly) attempt either way, with a \
                     summary on stderr.\n\
                     \n\
                     --strict changes only the exit code: if anomalies remain after the \
                     retry budget, rdg exits 1 instead of 0, so a CI or agent pipeline can \
                     hard-fail instead of silently shipping an imperfect diagram. Some \
                     anomaly kinds (an explicit size override, an extreme aspect ratio) \
                     aren't fixable by retrying spacing alone — they need a YAML change, \
                     which is exactly what a --strict failure here is telling you to make."
    )]
    strict: bool,

    /// Path to a design-tokens YAML file overriding rdg's base spacing/typography/
    /// threshold tokens, without recompiling. See `DesignTokens` in `rdg-layout` for
    /// every overridable field and its default.
    #[arg(
        long,
        value_name = "FILE",
        help = "Path to a design-tokens YAML file (base spacing/typography/threshold overrides)",
        long_help = "Nearly every spacing, sizing, and threshold decision rdg makes — how \
                     much clearance to leave between two boxes that just avoided overlapping, \
                     how far a flow-numbering badge sits from its edge, the density above which \
                     a graph gets force-directed layout instead of Sugiyama, and so on — derives \
                     at runtime from a small set of base design tokens (a spacing unit, a font \
                     size, a couple of ratios, a couple of algorithm-quality knobs), not from \
                     independent hardcoded pixel values. Every one of those tokens has a sane \
                     built-in default, and every one is overridable here without a rebuild.\n\
                     \n\
                     Pass a YAML file with any subset of fields to override — omitted fields \
                     keep their default:\n\
                     \n\
                       unit: 10.0              # base spacing unit (default 8.0)\n\
                       font_size: 13.0          # base body font size (default 12.0)\n\
                       overlap_clearance_units: 3.0   # breathing room after resolving an overlap\n\
                     \n\
                     Usage:\n\
                       rdg -i diagram.yaml -o out.svg --design-config spacious.yaml\n\
                     \n\
                     Field names and defaults are documented on `rdg_layout::DesignTokens`."
    )]
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
    /// Topology-dispatched: let `rdg_dispatch::dispatch` pick the framework.
    Auto,
    /// Hierarchical Sugiyama framework for DAGs — top-to-bottom flow.
    /// Automatically breaks cycles using a greedy Feedback Arc Set.
    Sugiyama,
    /// Barnes-Hut force-directed layout for dense or massive graphs.
    Force,
    /// fCoSE-style compound spring embedder for nested/disconnected structure.
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
    let payload = DiagramPayload::from_yaml(&yaml).context(
        "YAML payload does not match the rdg schema — run `rdg --help` or `rdg --example`",
    )?;

    // Priority: YAML payload > CLI flag
    let theme = payload.theme.as_deref().unwrap_or(&cli.theme);

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
    let tokens = load_design_tokens(cli.design_config.as_deref())?;

    // --- 3. Build petgraph --------------------------------------------------
    // Moved ahead of the spacing block below: computing a proportional
    // `rank_spacing`/`node_spacing` default needs this diagram's own node sizes,
    // which needs the compiled graph. `build_graph` only depends on `payload`, so
    // nothing else here needed to move with it.
    let compiled = build_graph(&payload)?;
    if compiled.had_cycles {
        eprintln!("⚠  Cycles detected in input graph — automatically broken via Feedback Arc Set.");
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
        report_dispatch(&decision, cli.layout == LayoutEngine::Auto);
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
    report_review(&reviewed);
    let remaining_anomalies = reviewed.remaining_anomalies(&compiled, &layout_config.tokens);
    if std::env::var("RDG_DEBUG_ANOMALIES").is_ok() {
        for a in &remaining_anomalies {
            eprintln!("  [{:?}] {}", a.kind, a.description);
        }
    }
    let layout_result = &reviewed.layout;
    let edge_plans = &reviewed.edge_plans;
    if std::env::var("RDG_DEBUG_SPACING").is_ok() {
        let m = rdg_render_core::review::spacing_metrics(&compiled, layout_result, edge_plans);
        let fmt = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{v:.1}"));
        eprintln!(
            "spacing: arrow={} node_gap={} port_pitch={}  [arrow: {}; pitch: {}]",
            fmt(m.min_arrow_len),
            fmt(m.min_node_gap),
            fmt(m.min_port_pitch),
            m.min_arrow_edge.as_deref().unwrap_or("-"),
            m.min_pitch_face.as_deref().unwrap_or("-")
        );
    }

    // --- 5. Render ----------------------------------------------------------
    let output_path = Path::new(&cli.output);
    let is_svg = output_path.extension().and_then(|e| e.to_str()) == Some("svg");

    if is_svg {
        let drawio_xml = render_drawio(&compiled, layout_result, edge_plans, theme, background, &layout_config.tokens)
            .context("draw.io XML rendering failed")?;

        let mut rendered_exact_drawio = false;
        if cli.svg_engine != "native" {
            if let Some(()) = try_export_svg_via_drawio_cli(&drawio_xml, output_path, theme) {
                rendered_exact_drawio = true;
            }
        }

        if !rendered_exact_drawio {
            if cli.svg_engine == "drawio" {
                anyhow::bail!(
                    "Exact draw.io export was requested (--svg-engine=drawio), but drawio CLI is not installed or failed"
                );
            }
            let svg_content = render_svg(&compiled, layout_result, edge_plans, theme, background, &layout_config.tokens)
                .context("SVG rendering failed")?;
            fs::write(&cli.output, svg_content)
                .with_context(|| format!("failed to write output to '{}'", cli.output))?;
        } else {
            eprintln!("✓ Rendered exact draw.io SVG export via drawio CLI");
        }
    } else {
        let content = render_drawio(&compiled, layout_result, edge_plans, theme, background, &layout_config.tokens)
            .context("draw.io XML rendering failed")?;
        fs::write(&cli.output, content)
            .with_context(|| format!("failed to write output to '{}'", cli.output))?;
    }

    eprintln!("✓ Diagram written to {}", cli.output);

    if cli.strict && !remaining_anomalies.is_empty() {
        std::process::exit(1);
    }
    Ok(())
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
fn report_dispatch(decision: &rdg_dispatch::AlgorithmDecision, was_auto: bool) {
    if !was_auto {
        return;
    }
    let framework = match decision.framework {
        rdg_dispatch::LayoutFramework::Sugiyama => "sugiyama",
        rdg_dispatch::LayoutFramework::ForceDirected => "force-directed",
        rdg_dispatch::LayoutFramework::FCose => "fcose",
    };
    let routing = match decision.routing.algorithm {
        rdg_dispatch::RoutingAlgorithm::CornerHeuristic => "corner heuristic",
        rdg_dispatch::RoutingAlgorithm::VisibilityGraphAStar => "visibility-graph A*",
    };
    eprintln!(
        "→  dispatch: {framework} ({}), routing: {routing} — {}",
        decision.complexity_estimate,
        decision.preprocessing_steps.join("; ")
    );
}

/// Prints a concise stderr summary of the self-review retry loop: nothing at all for a
/// clean first pass, one line per retry otherwise, and a final resolved/remaining summary.
fn report_review(reviewed: &rdg_render_core::review::ReviewedLayout) {
    if reviewed.passes.len() == 1 && reviewed.passes[0].anomaly_count == 0 {
        return;
    }
    for pass in &reviewed.passes {
        if pass.anomaly_count > 0 {
            eprintln!(
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
            );
        }
    }
    // The layout `rdg` actually writes is whichever pass had the *fewest* anomalies,
    // not necessarily the last one — widening spacing helps a layered Sugiyama layout
    // almost monotonically, but for the force-directed/fCoSE engines a wider spacing
    // scale can just as easily make crossings worse, so later passes aren't guaranteed
    // to improve on earlier ones. Reporting `passes.last()` here would describe a pass
    // whose result was silently discarded — this must match what `remaining_anomalies`
    // (and thus `--strict`) actually sees.
    let best_count = reviewed
        .passes
        .iter()
        .map(|p| p.anomaly_count)
        .min()
        .expect("at least one pass always runs");
    if best_count == 0 {
        if reviewed.passes.len() > 1 {
            eprintln!("✓  resolved after {} pass(es)", reviewed.passes.len());
        }
    } else {
        eprintln!(
            "✗  {} anomal{} {} in the best attempt tried ({} pass(es)) — writing it anyway",
            best_count,
            if best_count == 1 { "y" } else { "ies" },
            if best_count == 1 { "remains" } else { "remain" },
            reviewed.passes.len(),
        );
    }
}

/// Attempts to export exact draw.io SVG using the drawio desktop CLI if available in PATH or Applications.
fn try_export_svg_via_drawio_cli(drawio_xml: &str, output_path: &Path, theme: &str) -> Option<()> {
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

    let theme_arg = if theme == "dark" { "dark" } else { "light" };

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
