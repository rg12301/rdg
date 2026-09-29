# rdg — Render Diagram

> **Deterministic diagram compiler for LLM-generated semantic graphs.**  
> You describe *what* to draw in token-efficient YAML. `rdg` figures out *where* to draw it.

[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

---

## Why rdg?

Large Language Models are excellent at semantic reasoning but fail at 2D spatial layout. When forced to write raw draw.io XML or SVG, they produce:

- Overlapping nodes and crossing edges  
- Hallucinated pixel coordinates  
- Invalid XML that silently breaks draw.io  
- Massive token waste on geometry boilerplate  

**rdg decouples the two concerns:**

| You (the LLM) | rdg |
|---|---|
| Decide which nodes exist | Compute pixel-perfect x/y coordinates |
| Decide how they connect | Apply Sugiyama hierarchical layout |
| Choose semantic types | Map types to draw.io styles |
| Emit compact YAML | Serialise strict mxfile XML or SVG |

### What's new

- **Sequence diagrams, properly**: an ordered `sequence:` script with notes, fragments
  (`alt`/`else`, `opt`, `loop`, `par`/`and`, `critical`, `break`, `region`), dividers,
  participant groups, actors, automatic activation bars (a call with a reply activates
  the callee), `create`/`destroy` lifecycles and autonumbering. Lifelines are spaced to
  fit the labels, notes and fragment headers between them.
- **Logos with halos, centred cards**: a node drawn as its logo gets an invisible circle
  around the logo that arrows start and end on; a card's icon sits inline before its
  title and the two are centred together. Long node lines wrap into balanced lines;
  long edge labels wrap onto two lines, and `--check` asks agents to split them.

- **Themes with meaning**: every colour, font, size, shape and arrow style comes from a
  YAML theme — nothing design-related is hardcoded. Five built-ins: `light` (default,
  flat and minimal), `dark`, `mono-light`, `mono-dark` (black & white) and `classic`.
  Colour encodes a node's *category* (frontend, backend, data, messaging, edge/cloud,
  security, external), and a legend can list them. Infra with an official logo —
  Postgres, Redis, Kafka, … — is drawn as the enlarged full-colour logo instead of a box.

- **Search-based arrow routing**: every edge is routed by an orthogonal A* search over
  node outlines and channel midlines that picks the faces itself (instead of guessing
  them first), weighing length, bends, crossings, shared lanes and group borders; edges
  are then re-routed with all others in place, and parallel segments are fanned out into
  evenly spaced lanes. Arrows touch the real outline of ellipses, cylinders and diamonds.
  `RDG_LEGACY_ROUTER=1` switches back to the previous heuristic router for comparison.
- **Tiered group layout**: groups are stacked in rows by the flow between them (groups
  that call each other share a row, pure sources sit right above what they call, overly
  wide rows wrap), then slid toward what they connect to — so arrows between tiers stay
  short and straight.
- **Label and badge placement**: edge labels are placed beside a clear straight stretch
  of their own arrow (wrapped onto two lines when that fits better), scored against every
  node, line, group title and other label; flow-number badges sit beside their arrow
  near its start instead of on it. Node labels read as a bold title plus smaller detail
  lines, and icons sit inside their shape.
- **Agent-friendly CLI**: unknown YAML keys are errors with a did-you-mean; `--check`
  validates without writing and lists every problem (bad ids, unknown types/styles/icons)
  with its YAML path; `--format json` reports results as one JSON object;
  `--guide <topic>` and `--list-icons` replace the old 27 KB help page.
- **Topology-dispatched layout**: `rdg` inspects each input graph (cyclicity, edge
  density, compound/nested group structure, connected components) and automatically
  picks between three layout frameworks — the original Sugiyama layered layout, a
  Barnes-Hut force-directed engine, and an fCoSE-style compound spring embedder (edges
  are then routed by the search router above). `--layout auto` (the default) reports its choice
  on stderr; `--layout sugiyama|force|fcose` forces one explicitly. See
  [`crates/rdg-dispatch`](crates/rdg-dispatch) for the decision logic.
- **Self-review**: every render runs a deterministic geometry check (overlaps, edges
  cutting through nodes, an explicit size too small for its content, a lopsided canvas)
  and retries with wider spacing before writing anything; `--strict` turns unresolved
  anomalies into a non-zero exit code for CI/agent pipelines.
- **Consistent whitespace**: rank gaps, node gaps, group padding and the canvas margin
  scale with how connected a component is (arrows never shrink below a readable stub,
  parallel edges get their own lanes, a hub's faces are sized to hold its ports), and the
  configured margin holds on all four sides of everything drawn.
- **Polish pass**: the last stage reviews the routed diagram for small imperfections — an
  arrow with a needless micro-jog, an avoidable crossing, two edges drawn on top of each
  other — and fixes each with a small guarded adjustment (sliding a port along its face,
  swapping two edges' ports, moving one edge to another face). A fix is kept only if it
  breaks no rule (no node hit, no extra bend, no shorter arrow) and strictly improves the
  diagram, so the pass is deterministic and idempotent; nodes are never moved. It also
  snaps geometry to whole pixels so draw.io draws exactly what was routed. `--no-polish`
  turns it off; `RDG_DEBUG_POLISH=1` prints every adjustment.
- **Real brand logos**: ~60 languages/clouds/databases/frameworks now use vendored
  [Simple Icons](https://simpleicons.org) artwork (CC0) instead of hand-drawn
  approximations — see [`crates/rdg-icons/assets/THIRD_PARTY_LICENSES.md`](crates/rdg-icons/assets/THIRD_PARTY_LICENSES.md).
- **Flow numbering**: `numbered: true` puts a sequence badge (①②③…) on each edge in
  declaration order, so a reader can trace the flow.
- **Reusable style presets**: define a named `node_styles`/`edge_styles` preset once,
  reference it from any node/edge via `class:` instead of repeating the same overrides.
- **Chain straightening**: a lightweight coordinate-alignment pass keeps simple chains
  vertically/horizontally aligned instead of zigzagging, and centers a node over its
  children's span.
- **More granular control**: per-node `color`/`width`/`height`/`provider`/`link`
  overrides, a draw.io-only `style_extra` raw-style escape hatch, and `canvas`/`spacing`
  config objects for margin and gap tuning.

---

## Architecture

### High-Level Design

![HLD diagram](docs/hld.svg)

> Source: [`docs/hld.yaml`](docs/hld.yaml) — generated with `rdg --input docs/hld.yaml --output docs/hld.svg`

**Pipeline:**

```
LLM / Agent
    │  emits token-efficient YAML
    ▼
rdg CLI (--input / stdin)
    │
    ├─► Schema Parser    (serde_yaml → DiagramPayload)
    ├─► Graph Builder    (petgraph StableDiGraph + FAS cycle breaking)
    ├─► Layout Engine    (deterministic layered layout: topological ranking,
    │                     barycentric crossing minimisation, compound group grid)
    └─► Renderer (shared style/routing core + two backends)
            ├─► draw.io XML  (.drawio)
            └─► SVG          (.svg)
```

### Low-Level Design

![LLD diagram](docs/lld.svg)

> Source: [`docs/lld.yaml`](docs/lld.yaml) — generated with `rdg --input docs/lld.yaml --output docs/lld.svg`

**Crate breakdown** (`rdg` is a Cargo workspace, `crates/*`):

| Crate | Responsibility |
|---|---|
| `rdg-schema` | `serde` structs for YAML input: `DiagramPayload`, `NodeDef`, `EdgeDef` |
| `rdg-graph` | Build `StableDiGraph<NodeData, EdgeData>`; Eades–Lin–Smyth greedy Feedback Arc Set cycle-breaker |
| `rdg-layout` | Deterministic layered layout: topological ranking, barycentric crossing minimisation, compound 2D group grid placement |
| `rdg-icons` | Built-in flat vector icons for languages/databases; language & DB engine detection heuristics |
| `rdg-render-core` | Style/typography/routing shared by both render backends, so they can't drift apart on what a node type or edge style means |
| `rdg-render-drawio` | draw.io (`mxfile`) XML backend |
| `rdg-render-svg` | SVG backend |
| `rdg-cli` | The `rdg` binary: CLI parsing, I/O, ties the pipeline together |

---

## Installation

### Prerequisites

- [Rust](https://rustup.rs) 1.85 or later
- Cargo (bundled with Rust)

### Build from source

```bash
git clone https://github.com/rg12301/rdg.git
cd rdg
cargo build --release
```

The binary is at `target/release/rdg`. Copy it anywhere on your `$PATH`:

```bash
# macOS / Linux
sudo cp target/release/rdg /usr/local/bin/rdg

# Or without sudo, into your home bin:
cp target/release/rdg ~/.local/bin/rdg
```

### Download precompiled binary

**Recommended:** grab a precompiled binary for your platform from the [latest release](https://github.com/rg12301/rdg/releases):

- **macOS**: `rdg-macos-x86_64` (Intel) or `rdg-macos-aarch64` (Apple Silicon)
- **Linux**: `rdg-linux-x86_64` (glibc, x86_64) or `rdg-linux-aarch64` (ARM64)
- **Windows**: `rdg-windows-x86_64.exe`

Unzip/untar and place the binary on your `$PATH`:

```bash
# macOS / Linux example:
tar xzf rdg-macos-aarch64.tar.gz
sudo mv rdg /usr/local/bin/

# Or without sudo:
mv rdg ~/.local/bin/
```

### Verify installation

```bash
rdg --version
# rdg 1.1.1

rdg --help
# Prints the full LLM-friendly usage guide
```

---

## Usage

### Input schema (YAML)

```yaml
diagram_type: flowchart        # required — logical category
theme: light                   # optional — built-in name, or a mapping that overrides one
legend: true                   # optional — list the colour categories used
groups:                        # optional — visual containers
  - id: g1
    label: "Ingestion Tier"
    nodes: [n1, n2]
nodes:
  - id: n1                     # required — unique, short, no spaces
    label: "API Gateway\nv2.4" # required — bold title + detail lines
    type: gateway              # optional — picks category, shape and glyph
  - id: n2
    label: "orders"
    type: database
    db_type: postgres          # official logo; database-like types render as the logo
    display: card              # optional — force a card instead of the logo
edges:
  - from: n1                   # source node id
    to: n2                     # target node id
    label: "SQL queries"       # optional edge label
    edge_style: data           # optional: flow | async | data | auth | error | bidirectional | ...
```

### Theming

Run `rdg --list-themes`, `rdg --print-theme light` (the full, commented YAML) and
`rdg --guide theming`. Pick a theme with `theme:` in the YAML or `--theme <name|file.yaml>`
(the CLI wins). A theme file can `extends: dark` and override only what it changes;
a `theme:` *mapping* in the diagram YAML is deep-merged over the chosen theme:

```yaml
theme:
  extends: dark
  font: {family: "Inter, sans-serif"}
  categories:
    database: {stroke: "#22c55e"}
  edge_styles:
    async: {dash: "2 2"}
```

**Colour = meaning.** A node's colour is its category, taken from `category:` or its
`type` (via the theme's `type_categories`) — and nothing else: containers are drawn
neutral so they never compete with the nodes inside them, category glyphs are left out
of colour themes (the colour already says it; black-and-white themes use the glyphs
instead), and a legend explains the colours whenever there are at least two. A
hand-picked `color:` on a node, group or edge still works, but `rdg --check` warns,
because it breaks the code. Every diagram type — architecture, flowchart, ER, class,
state, sequence — is coloured by the same rules unless the YAML customises it:

| Category | Default types | Light theme stroke |
|---|---|---|
| `frontend` | client, user, browser, frontend | blue `#2563eb` |
| `backend` | service, server, backend, function, lambda | green `#059669` |
| `database` | database, db, storage, table, cache, redis | violet `#7c3aed` |
| `messagebus` | queue, broker, bus | orange `#ea580c` |
| `cloud` | proxy, gateway, api, cdn | teal `#0891b2` |
| `security` | auth, firewall | red `#dc2626` |
| `external` | external, third_party | slate `#64748b` |
| `neutral` | anything else | grey `#94a3b8` |

Edge styles also carry meaning: `async` is dashed in the messaging colour with an open
head, `data`/`stream` is thicker in the data colour, `auth` and `error`/`fallback` use
the security colour; ER (`one_to_many`, …) and UML (`inheritance`, `composition`, …)
ends are built in. Shapes (`database` → cylinder, `decision` → diamond), icon sizes,
which types render as a bare logo (`icon.node_types`), the icon style (`color` or
`mono`) and every font size and colour live in the theme too.

Brand logos are the full-colour [devicon](https://devicon.dev) set (MIT); see
[`crates/rdg-icons/assets/THIRD_PARTY_LICENSES.md`](crates/rdg-icons/assets/THIRD_PARTY_LICENSES.md).

### Sequence diagrams

```yaml
title: Checkout
numbered: true
participants:
  - {id: user, label: Shopper, type: actor}
  - {id: web, label: Web App, type: client}
  - {id: api, label: Orders API, type: service}
  - {id: db, label: orders, type: database, db_type: postgres}
groups:
  - {id: backend, label: Backend, nodes: [api, db]}
sequence:
  - {from: user, to: web, label: "place order"}
  - {from: web, to: api, label: "POST /orders"}
  - {note: "idempotency key checked", right_of: api}
  - alt: "in stock"
    steps:
      - {from: api, to: db, label: "INSERT order"}
      - {from: db, to: api, label: "id", style: reply}
      - {from: api, to: web, label: "201 Created", style: reply}
    else:
      label: "out of stock"
      steps:
        - {from: api, to: web, label: "409 Conflict", style: reply}
  - divider: "later"
  - loop: "every 30s"
    steps:
      - {from: web, to: api, label: "GET /orders/:id"}
```

Each step is one of: a message (`from`, `to`, `label`, `style: flow|reply|async`,
`activate`, `deactivate`, `create`, `destroy`), a note (`note` + `over`/`left_of`/
`right_of`), a fragment (`alt`, `opt`, `loop`, `par`, `critical`, `break`, `region`
with nested `steps`, plus `else` / `and` branches) or a `divider`. Activation bars are
automatic: a call opens one on the receiver when a reply comes back later. Colours and
spacing live in the theme's `sequence:` section. `rdg --guide sequence` has the details;
[`examples/login_sequence/login_sequence.yaml`](examples/login_sequence/login_sequence.yaml) uses every feature.

### CLI flags

```
rdg [OPTIONS] [INPUT]

  -i, --input <FILE>         YAML input (omit or - for stdin)
  -o, --output <FILE>        Output; extension picks the format (.drawio | .svg) [default: output.drawio]
      --check                Validate only: list every error/warning, write nothing
      --format <text|json>   Diagnostics as text on stderr, or one JSON object on stdout
      --guide <TOPIC>        schema | nodes | edges | groups | styles | theming | sequence | typography | layout | output | all
      --list-icons           Every icon key for icon:/language:/db_type:
      --example / --schema   Commented YAML template / JSON Schema
  -l, --layout <LAYOUT>      auto | sugiyama | force | fcose [default: auto]
  -t, --theme <NAME|FILE>    light | dark | mono-light | mono-dark | classic, or a theme YAML
      --list-themes          Built-in themes, one per line
      --print-theme <NAME>   Print a theme's full YAML (a starting point for your own)
      --direction <DIR>      tb | lr [default: tb]
      --rank-spacing <PX>    Gap between ranks (default: proportional to node size)
      --node-spacing <PX>    Gap between neighbouring nodes (default: proportional)
      --strict               Exit 1 if layout anomalies remain (the file is still written)
      --no-polish            Skip the final polish pass
      --svg-engine <ENGINE>  auto | drawio | native [default: auto]
      --design-config <FILE> Design-token overrides (spacing unit, font size, thresholds)
```

### Examples

```bash
# Minimal — read from stdin, write draw.io
rdg --output architecture.drawio << 'EOF'
diagram_type: flowchart
nodes:
  - id: n1
    label: "Client"
    type: client
  - id: n2
    label: "API"
    type: proxy
  - id: n3
    label: "DB"
    type: database
edges:
  - from: n1
    to: n2
    label: "HTTPS"
  - from: n2
    to: n3
    label: "SQL"
EOF

# From file → SVG with custom spacing
rdg --input diagram.yaml --output diagram.svg --rank-spacing 80 --node-spacing 50

# Pipe from LLM output → draw.io → open in browser
cat llm_output.yaml | rdg -o diagram.drawio && open diagram.drawio
```

### Output formats

| Extension | Format | Open with |
|---|---|---|
| `.drawio` | Uncompressed mxfile XML | [draw.io desktop](https://github.com/jgraph/drawio-desktop), [diagrams.net](https://app.diagrams.net) |
| `.svg` | Scalable Vector Graphics | Any browser, embed in Markdown / HTML |

---

## For LLM Agents

`rdg --help` is written for agents: one screen with the workflow, a minimal YAML
example, every node/edge/top-level field, node types and edge styles, and exit codes.
`rdg --guide <topic>` has the detail. Mistakes fail loudly instead of rendering
something wrong: an unknown key, a typo'd node id or an undefined preset is an error
naming its YAML path and the closest valid name, and `--format json` makes that
machine-readable:

```
$ rdg --check --format json -i diagram.yaml
{"ok":false,"output":null,"layout":null,
 "errors":[{"path":"edges[3].to","message":"unknown node id `auth_svc`","hint":"did you mean `auth_api`?"}],
 "warnings":[{"path":"nodes[1].type","message":"unknown node type `servce` renders as a plain card","hint":"did you mean `service`?"}],
 "anomalies":[]}
```

**Recommended system prompt addition:**

```
You have access to the `rdg` CLI. To draw an architecture or flow diagram, write the
diagram as rdg YAML (read `rdg --help` once for the format), then run
  rdg --check --format json -i diagram.yaml     # fix every error it lists
  rdg -i diagram.yaml -o diagram.drawio
Never compute coordinates or write draw.io XML/SVG yourself.
```

---

## Generating the design diagrams

The HLD and LLD for this project were generated using `rdg` itself:

```bash
# High-Level Design
rdg --input docs/hld.yaml --output docs/hld.drawio
rdg --input docs/hld.yaml --output docs/hld.svg

# Low-Level Design
rdg --input docs/lld.yaml --output docs/lld.drawio --rank-spacing 90 --node-spacing 60
rdg --input docs/lld.yaml --output docs/lld.svg    --rank-spacing 90 --node-spacing 60
```

---

## Project structure

```
rdg/
├── Cargo.toml
├── README.md
├── docs/
│   ├── hld.yaml        # HLD input (edit to update diagram)
│   ├── hld.drawio      # Generated HLD — draw.io format
│   ├── hld.svg         # Generated HLD — SVG
│   ├── lld.yaml        # LLD input
│   ├── lld.drawio      # Generated LLD — draw.io format
│   └── lld.svg         # Generated LLD — SVG
└── src/
    ├── main.rs         # CLI entry point (clap)
    ├── lib.rs          # Module root
    ├── schema.rs       # YAML input structs (serde)
    ├── graph.rs        # Graph construction + FAS cycle breaking (petgraph)
    ├── layout.rs       # Spatial layout (layout-rs / topo-sort fallback)
    └── render.rs       # draw.io XML + SVG serialisation (quick-xml)
```

---

## Running tests

```bash
cargo test
# 19 tests — schema, graph, layout, render
```

---

## Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `clap` | 4.6 | CLI argument parsing with derive macros |
| `serde` + `serde_yaml` | 1 / 0.9 | YAML deserialization into typed structs |
| `petgraph` | 0.6 | `StableDiGraph` — stable node indices for layout |
| `layout-rs` | 0.1 | Sugiyama hierarchical layout engine |
| `quick-xml` | 0.36 | Low-overhead XML writer for mxfile + SVG |
| `anyhow` | 1 | Application error handling |
| `thiserror` | 2 | Typed library error definitions |

---

## License

MIT © rdg contributors
