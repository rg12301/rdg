//! Help text: the short `--help` an agent reads first, and the `--guide <topic>` pages
//! it can pull for detail. Kept plain (no box-drawing art) and factual — every field,
//! type and style named here is one the parser and renderers actually accept, and
//! `rdg --check` enforces the same rules.

use clap::ValueEnum;

/// `rdg --help`: everything needed for a typical diagram, in one screen.
pub const LONG_ABOUT: &str = "\
rdg compiles a YAML description of a diagram (nodes, edges, groups) into a laid-out
draw.io (.drawio) or SVG (.svg) file. You describe WHAT to draw; rdg decides WHERE:
positions, sizes, arrow routes, label and badge placement. Never write coordinates.

WORKFLOW
  1. Write YAML (template: `rdg --example`).
  2. Validate:  rdg --check -i diagram.yaml          (writes nothing; lists every problem)
  3. Render:    rdg -i diagram.yaml -o diagram.drawio   (or .svg)
  Add `--format json` to either for machine-readable results on stdout.

MINIMAL YAML
  title: \"Checkout Service\"              # optional banner; `description:` adds a subtitle
  direction: tb                          # tb (top-to-bottom, default) | lr
  numbered: true                         # optional: number edges 1,2,3… in declaration order
  groups:                                # optional containers
    - {id: core, label: \"Core Services\", nodes: [api, db]}
  nodes:
    - id: api                            # required, unique, no spaces
      label: \"Order API\\nREST · Port 8080\" # first line = bold title, next lines = details
      type: service                      # see NODE TYPES
      technology: \"Axum\"                 # shown as [Axum]
      language: rust                     # icon (see `rdg --list-icons`)
    - {id: db, label: \"orders\", type: database, db_type: postgres}
  edges:
    - {from: api, to: db, label: \"SQL\", edge_style: data}

NODE FIELDS   id, label, type, category, display (card|icon), technology, language,
              db_type, icon, provider (aws|gcp), metadata (tooltip), fields (table rows /
              class members), color, width, height, link, class (a node_styles preset),
              style_extra (raw draw.io style)
EDGE FIELDS   from, to, label, edge_style, color, width, line_style (solid|dashed|dotted),
              head, tail, source_port / target_port (top|bottom|left|right), step,
              class (an edge_styles preset), style_extra
TOP LEVEL     title, description, diagram_type, direction, theme, legend, numbered,
              groups, nodes, edges, node_styles, edge_styles, canvas {margin, background},
              spacing {rank, node, group_gap_x, group_gap_y}
Unknown keys are errors (with a did-you-mean), so a typo can't silently drop a field.

NODE TYPES    service|server|backend  proxy|gateway|api  database|db|storage (cylinder)
              table|entity|record (cylinder)  queue|broker|bus (ellipse)  cache|redis
              function|lambda|faas  client|user|browser  decision|condition (diamond)
              start  end  choice  class|interface|abstract_class|struct  participant|actor
              Anything else renders as a neutral card (and --check warns).
EDGE STYLES   flow (default)  async  sync|call  reply|return  data|stream  error|fallback
              bidirectional  one_to_many  many_to_many  one_to_one  zero_to_many
              inheritance  realization  composition  aggregation  dependency

DIAGRAM TYPES architecture | flowchart | sequence | erd | class | state
SEQUENCE      `participants:` + an ordered `sequence:` of messages, notes, fragments
              (alt/else, opt, loop, par/and, critical, break, region) and dividers;
              activations, create/destroy and numbering are automatic or per message.
              See `rdg --guide sequence`.

COLOUR = MEANING  Every node has a category (from its type, or `category:`): frontend,
              backend, database, messagebus, cloud, security, external, neutral. The
              category alone sets its colour; edge styles reuse them (async = messaging
              hue, data = database hue, error/auth = security hue), and a legend
              explains them. Containers stay neutral. So: pick the right `type` (or
              `category:`), and do NOT set `color:` to tell boxes or groups apart —
              five services are five green boxes, and that is correct (--check warns).
              Icons mean something too: a logo shows the technology; a group's language
              icon replaces the same icon on each member. Infra with a logo (postgres,
              redis, kafka, …) is drawn as its logo; `display: card` keeps a box.
THEMES        --theme light|dark|mono-light|mono-dark|classic, or a theme YAML file.
              `rdg --guide theming` explains overriding any colour/font/size.

LABEL TIPS    Split long text yourself with \\n — long single lines eat the whitespace
              arrows need and push groups apart.
              Edge labels: at most ~22 characters per line, at most 2 lines; name the
                protocol or action (\"gRPC\", \"publish order\"). Good:
                \"Verify Credentials\\n& User State\". Bad: \"Verify Credentials & User State\".
                (Unsplit labels over 22 chars are wrapped onto two lines for you.)
              Node labels: a short title (≤ 22 chars) plus 1-2 detail lines of
                ≤ 26 chars — \"auth-api-ext\\nPublic Auth APIs\\ncalled by Web & Mobile\".
              Put long explanations in `metadata` (shown as a tooltip).

EXIT CODES    0 written (warnings may be printed)   1 invalid input or failure
              With --strict, also 1 when layout anomalies remain.

MORE          rdg --guide <schema|nodes|edges|groups|styles|theming|typography|layout|output|all>
              rdg --example   rdg --schema (JSON Schema)   rdg --list-icons";

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic {
    /// Every field, with aliases.
    Schema,
    /// Node types, shapes, icons, sizing.
    Nodes,
    /// Edge styles, arrows, numbering, port hints.
    Edges,
    /// Groups/containers and how they are arranged.
    Groups,
    /// Reusable node_styles / edge_styles presets.
    Styles,
    /// Themes: built-ins, overriding colours/fonts/sizes, custom theme files.
    Theming,
    /// Sequence diagrams: messages, activations, notes, fragments, groups.
    Sequence,
    /// Inline formatting in labels.
    Typography,
    /// Layout engines, routing, spacing, self-review.
    Layout,
    /// Output formats, --format json, exit codes.
    Output,
    /// All of the above.
    All,
}

const SEQUENCE: &str = "\
SEQUENCE DIAGRAMS

Declare participants left to right (`participants:` = `nodes:`), then the script in
order under `sequence:`. Setting `sequence:` makes the diagram a sequence diagram.

  title: Checkout
  numbered: true                       # prefix messages with 1., 2., …
  participants:
    - {id: user, label: Shopper, type: actor}      # stick figure (theme actor_types)
    - {id: web, label: Web App, type: client}
    - {id: api, label: Orders API, type: service}
    - {id: db, label: orders, type: database, db_type: postgres}
  groups:                              # boxes around participants (kept adjacent)
    - {id: backend, label: Backend, category: backend, nodes: [api, db]}
  sequence:
    - {from: user, to: web, label: \"place order\"}
    - {from: web, to: api, label: \"POST /orders\"}
    - {note: \"idempotency key checked\", right_of: api}
    - alt: \"in stock\"
      steps:
        - {from: api, to: db, label: \"INSERT order\"}
        - {from: db, to: api, label: \"id\", style: reply}
        - {from: api, to: web, label: \"201 Created\", style: reply}
      else:
        label: \"out of stock\"
        steps:
          - {from: api, to: web, label: \"409 Conflict\", style: reply}
    - divider: \"later\"
    - loop: \"every 30s\"
      steps:
        - {from: web, to: api, label: \"GET /orders/:id\"}

Step kinds (exactly one per step)
  message    from, to, label (text), style (flow | reply | return | async), color,
             activate, deactivate, create, destroy
  note       note: text, plus over: id | [id, id] | left_of: id | right_of: id
  fragment   alt | opt | loop | par | critical | break | region: \"guard\",
             steps: [...], else: {label, steps} or a list (alt), and: … (par)
  divider    divider: \"label\"   (a full-width separator)

Messages
  style: reply (or return) is a dashed return arrow; async an open-headed one.
  A self-message (from == to) draws a loop with its label on the right.
  create: true — the receiver's box appears at this message instead of the top row.
  destroy: true — the receiver's lifeline ends here with an ✕.

Activations (the bars on lifelines)
  Automatic: a call opens a bar on the receiver when a reply to it comes later, and
  that reply closes it; bars nest. Arrows meet the bar's edge.
  activate: true / false on a message forces or suppresses the receiver's bar (on a
  self-message: a short nested bar); deactivate: true ends the sender's bar.

Layout
  Lifelines are spaced to fit each message label, note and fragment header between
  them — no widths needed. Keep message labels on one line (they run along the
  arrow); split notes with \\n or let them wrap (theme sequence.note.wrap_chars).
  Colours, spacing, note/fragment styles: the theme's `sequence:` section.

Plain `edges:` (in order) still work as a message list without fragments or notes.
";

const SCHEMA: &str = "\
SCHEMA — every accepted key (aliases in parentheses). Unknown keys are rejected.

Top level
  title (name)                 banner text above the diagram
  description (subtitle, desc) one-line subtitle under the title
  diagram_type (type, kind)    architecture | flowchart | sequence | erd | class | state
  direction (flow)             tb | lr
  theme                        light | dark | mono-light | mono-dark | classic,
                               or {extends: <name>, …overrides} (see --guide theming)
  legend                       true: legend of the categories and edge styles used
  numbered                     true: badge each edge with its step number
  rank_spacing, node_spacing   px overrides (default: proportional to node size)
  canvas                       {margin: px, background: \"#hex\"}
  spacing                      {rank, node, group_gap_x, group_gap_y}
  node_styles, edge_styles     named presets (see --guide styles)
  groups (containers, swimlanes), nodes (participants), edges
  sequence                     sequence-diagram script (see --guide sequence)

Node
  id                            required, unique across nodes and groups
  label                         text; line 1 = title, further lines = details
  title (name), subtitle        alternative to label: title + one detail line
  type (kind, shape)            see --guide nodes
  category                      colour by meaning: frontend, backend, database,
                                messagebus, cloud, security, external, neutral
  display (render_as)           card (a box) | icon (the logo, label underneath)
  technology (tech, stack)      rendered as a [bracketed] detail line
  language (lang, runtime)      language icon: rust, go, python, typescript, java, …
  db_type (engine, db)          database icon: postgres, mysql, redis, mongodb, kafka, …
  icon (logo, badge)            any key from `rdg --list-icons`
  provider (cloud)              aws | gcp
  metadata (tooltip, desc)      hover text
  fields (columns, attributes)  rows for table/entity/class cards
  color (stroke, accent)        border colour override, \"#hex\" — replaces the
                                category colour; avoid unless the colour means something
  width, height                 px size override (rdg sizes nodes itself otherwise)
  link (url, href)              click-through URL
  class (preset, style)         name of a node_styles preset
  style_extra (drawio_style)    raw draw.io style fragment, appended verbatim

Edge
  from (source, src), to (target, dst)   node ids (required)
  label (text)                  short text placed beside the line; ≤ 22 chars per
                                line, split longer ones with \\n (max 2 lines)
  edge_style (style, type)      see --guide edges
  bidirectional (bidir)         arrows on both ends
  color, width, line_style      stroke overrides; line_style: solid | dashed | dotted
  head, tail                    arrowhead overrides (draw.io marker names, e.g. open, block, none)
  source_port, target_port      force the face: top | bottom | left | right
  step (order, sequence)        explicit number for `numbered: true`
  class (preset), style_extra

Group
  id, label (title, name), nodes (members, node_ids), category, color, language, icon
";

const NODES: &str = "\
NODES

Types → category (colour) and shape, in the default theme
  service | server | backend | function | lambda   backend      card
  proxy | gateway | api | cdn                       cloud        card
  database | db | storage | table | entity          database     cylinder
  cache | redis | memcache                          database     card
  queue | broker | bus                              messagebus   card
  client | user | browser | frontend                frontend     card
  auth | firewall                                   security     card
  external | third_party                            external     card
  decision | condition                                           diamond
  Any node can set `category:`; themes can remap types and shapes.
  start, end, choice             small markers (label drawn below)
  class | interface | abstract_class | struct   UML card with `fields`
  participant | actor            sequence-diagram participants
  anything else                  neutral card (rdg --check warns)

Labels
  The first line of `label` is the bold title; later lines (split with \\n) are
  smaller detail lines. Lines in (parentheses) or [brackets] are always details.
  rdg sizes every node to fit its text; set width/height only when you must.
  Keep the title ≤ 22 chars and each detail line ≤ 26; split longer text with \\n
  yourself. Lines that are still too long are wrapped into balanced lines.

Icons
  Chosen from, in order: `icon`, `language`, `db_type`, `provider`, then a language
  named in technology/label/metadata; cards without one get their category glyph.
  Drawn inline before the node's title. Databases, caches and queues with a real
  logo (db_type: postgres, redis, kafka, …) are drawn *as* the logo with the label
  below; arrows attach to an invisible circle around the logo (theme `icon.halo`).
  `display: card` keeps a box, `display: icon` forces the logo.
  `rdg --list-icons` prints every key.
";

const EDGES: &str = "\
EDGES

Styles (edge_style)
  flow (default)        solid line, filled arrow
  async                 dashed amber line, open arrow (events, queues)
  sync | call           synchronous call
  reply | return        dashed return arrow
  data | stream         thicker indigo line (reads/writes, replication)
  error | fallback      dashed red line, hollow arrow
  bidirectional         arrows at both ends
  one_to_many, many_to_many, one_to_one, zero_to_many      ER crow's-foot ends
  inheritance, realization, composition, aggregation, dependency   UML ends

Placement (automatic)
  Arrows are routed orthogonally around every node and group title, choosing the
  faces, with few bends and crossings; parallel arrows get separate lanes. Labels are
  placed beside a clear straight stretch of their arrow, wrapped onto two lines when
  that fits better. `numbered: true` puts a step badge beside each arrow near its
  start (numbering follows declaration order unless an edge sets `step`).

Overrides
  source_port / target_port: top | bottom | left | right — pin an end to one face.
  color, width, line_style, head, tail, style_extra — per-edge styling.
";

const GROUPS: &str = "\
GROUPS

  groups:
    - {id: data, label: \"Data Tier\", nodes: [db, cache]}

Containers are drawn neutral so the colours of the nodes inside them keep their
meaning. Set `category:` on a group only when everything in it is one kind (it is
then tinted in that category's colour); avoid `color:` (--check warns).
Each node belongs to at most one group. Groups are arranged in rows by the flow
between them: a group sits below the groups that feed it, groups that call each
other both ways share a row, and a row that gets too wide wraps. Within a row, each
group slides toward what it connects to, so arrows between tiers stay short.
Ungrouped nodes are placed the same way, without a container.
`language`/`icon` on a group shows that icon on its title instead of on every member.
";

const STYLES: &str = "\
STYLES (reusable presets)

  node_styles:
    payment: {category: security, technology: \"Rust\"}
  edge_styles:
    event: {edge_style: async, width: 1.5}
  nodes:
    - {id: pay, label: Payments, class: payment}
  edges:
    - {from: pay, to: bus, class: event}

A preset fills in only what the node/edge leaves unset. Node presets accept color,
technology, language, db_type, icon, width, height, provider, style_extra; edge presets
accept edge_style, color, width, line_style, head, tail, style_extra.
A `class` naming an undefined preset is an error.
";

const THEMING: &str = "\
THEMING

Built-in themes (`rdg --list-themes`):
  light       flat, colour by meaning, monospace (default)
  dark        the same meanings on a midnight canvas
  mono-light  black on white: categories keep their glyphs, edges their dashes
  mono-dark   white on black
  classic     elevated white cards with coloured borders, Inter type

Pick one:   theme: dark            (YAML)     --theme dark      (CLI; wins over YAML)
Override:   theme:
              extends: dark
              font: {family: \"Inter, sans-serif\", node_title_size: 13}
              categories:
                security: {stroke: \"#f43f5e\"}
              edge_styles:
                async: {dash: \"2 3\"}
Custom:     rdg --print-theme light > my-theme.yaml   (edit anything)
            rdg -i diagram.yaml --theme my-theme.yaml

Every visual value lives in the theme: canvas background and grid, text colours,
fonts and sizes, node corner radius / stroke / shadow, group border, edge colour /
width / arrowheads / label colours, badge colours and size, icon style (colour logos
or mono glyphs) and sizes, which node types are drawn as their logo, the category
list with each hue and glyph, type → category and type → shape maps, and each edge
style's colour (a hex value or @category), dash and ends. A theme file may itself
`extends:` a built-in. Unknown keys are errors.
";

const TYPOGRAPHY: &str = "\
TYPOGRAPHY (in node, edge and group labels)

  `code`        monospace          **bold**      bold
  *italic*      italic             __under__     underline
  ~~strike~~    strikethrough      H~2~O         subscript
  O(N^2^)       superscript        $\\alpha$      LaTeX math (MathJax in draw.io)
  \\n            line break (node labels: first line title, rest details)
";

const LAYOUT: &str = "\
LAYOUT

  --layout auto (default) inspects the graph and picks: layered (hierarchies, flows,
  grouped architectures), force (dense webs, >2 edges per node), fcose (disconnected
  components). The choice is printed to stderr.
  direction: tb | lr sets the main flow.
  Spacing scales with node size; override with rank_spacing / node_spacing (px).

Self-review
  After layout rdg checks for overlapping nodes, arrows through nodes, arrows drawn on
  top of each other and zig-zag routes, and retries with wider spacing if needed.
  It always writes its best attempt; --strict makes remaining problems exit 1.
";

const OUTPUT: &str = "\
OUTPUT

  -o file.drawio   editable draw.io XML (open in draw.io / diagrams.net)
  -o file.svg      SVG (uses the draw.io CLI when installed, else the built-in renderer;
                   force with --svg-engine drawio|native)

--format json prints one JSON object to stdout when rdg finishes:
  {\"ok\": true, \"output\": \"out.drawio\", \"layout\": \"sugiyama\",
   \"errors\": [{\"path\": \"edges[2].to\", \"message\": \"unknown node id `dbb`\",
               \"hint\": \"did you mean `db`?\"}],
   \"warnings\": [...], \"anomalies\": [{\"kind\": \"EdgeThroughNode\", \"message\": \"...\"}]}
  `ok` is false when nothing was written. With --check, `output` is null.

Exit codes: 0 success; 1 invalid input or failure (or anomalies, with --strict).
";

pub fn text(topic: Topic) -> String {
    match topic {
        Topic::Schema => SCHEMA.into(),
        Topic::Nodes => NODES.into(),
        Topic::Edges => EDGES.into(),
        Topic::Groups => GROUPS.into(),
        Topic::Styles => STYLES.into(),
        Topic::Theming => THEMING.into(),
        Topic::Sequence => SEQUENCE.into(),
        Topic::Typography => TYPOGRAPHY.into(),
        Topic::Layout => LAYOUT.into(),
        Topic::Output => OUTPUT.into(),
        Topic::All => [SCHEMA, NODES, EDGES, GROUPS, STYLES, THEMING, SEQUENCE, TYPOGRAPHY, LAYOUT, OUTPUT].join("\n"),
    }
}
