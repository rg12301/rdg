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
    ├─► Layout Engine    (layout-rs Sugiyama framework)
    └─► Renderer
            ├─► draw.io XML  (.drawio)
            └─► SVG          (.svg)
```

### Low-Level Design

![LLD diagram](docs/lld.svg)

> Source: [`docs/lld.yaml`](docs/lld.yaml) — generated with `rdg --input docs/lld.yaml --output docs/lld.svg`

**Module breakdown:**

| Module | File | Responsibility |
|---|---|---|
| `schema` | `src/schema.rs` | `serde` structs for YAML input: `DiagramPayload`, `NodeDef`, `EdgeDef` |
| `graph` | `src/graph.rs` | Build `StableDiGraph<NodeData, EdgeData>`; greedy DFS Feedback Arc Set cycle-breaker |
| `layout` | `src/layout.rs` | Run layout-rs Sugiyama engine; extract `Position::bbox()` per node; topo-sort fallback |
| `render` | `src/render.rs` | `quick-xml` Writer for mxfile XML; hand-rolled SVG; semantic type → style mapping |

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

### Verify installation

```bash
rdg --version
# rdg 1.0

rdg --help
# Prints the full LLM-friendly usage guide
```

---

## Usage

### Input schema (YAML)

```yaml
diagram_type: flowchart        # required — logical category
theme: standard                # optional — overrides --theme flag
nodes:
  - id: n1                     # required — unique, short, no spaces
    label: "API Gateway"       # required — text inside the shape
    type: proxy                # optional — controls shape + colour
    metadata: "Routes traffic" # optional — tooltip annotation
  - id: n2
    label: "Database"
    type: database
edges:
  - from: n1                   # source node id
    to: n2                     # target node id
    label: "SQL queries"       # optional edge label
```

### Node types

| `type` value | Shape | Colour |
|---|---|---|
| `proxy` / `gateway` / `api` | Rounded box | Blue |
| `server` / `service` / `backend` | Box | Green |
| `database` / `db` / `storage` | Cylinder | Blue |
| `queue` / `broker` / `bus` | Queue shape | Yellow |
| `cache` / `redis` | Diamond | Red |
| `function` / `lambda` / `faas` | AWS Lambda icon | Orange |
| `client` / `user` / `browser` | Person icon | Grey |
| `decision` / `condition` | Diamond | Yellow |
| *(anything else)* | Rounded box | White |

### CLI flags

```
rdg [OPTIONS]

Options:
  -i, --input <FILE>            YAML input file (omit or use - for stdin)
  -o, --output <FILE>           Output path; extension selects format [default: output.drawio]
  -l, --layout <LAYOUT>         sugiyama | orthogonal | organic [default: sugiyama]
  -t, --theme <THEME>           standard | aws | azure [default: standard]
      --rank-spacing <N>        Vertical gap between layers in px [default: 60]
      --node-spacing <N>        Horizontal gap between nodes in px [default: 40]
  -h, --help                    Print full LLM usage guide
  -V, --version                 Print version
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

Run `rdg --help` from your tool-use environment. The full help text contains:

- Complete YAML schema with rules and examples  
- All node type aliases with visual descriptions  
- An explicit 4-step agentic workflow  
- `DO NOT` guardrails (no coordinate hallucination, no raw XML)  
- Pipe-friendly usage for agentic pipelines  

**Recommended system prompt addition:**

```
You have access to the `rdg` CLI tool. When asked to generate architecture or
flow diagrams, emit ONLY a YAML block following the rdg schema, then call:
  rdg --input <file> --output <file>.drawio
Never compute pixel coordinates. Never write draw.io XML or SVG directly.
Run `rdg --help` to read the full schema and node type reference.
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
