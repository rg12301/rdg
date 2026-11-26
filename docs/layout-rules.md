# rdg layout rules — a specification for diagrams that stay readable as they grow

Status: proposal, written 2026-11-26 against commit `6ccd100` (v1.3.0). It is the result of
(a) rendering all 45 bundled fixtures and inspecting the six largest by eye, (b) reading the
layout, routing, label and review code, and (c) a survey of the graph-drawing literature and
the industrial engines (dot, ELK, yFiles, libavoid, draw.io, Mermaid).

The complaint it answers: small diagrams look good; above about 30 nodes the arrows tangle,
labels float between arrows, and a reader can no longer tell which line a label or arrowhead
belongs to. Every rule below is stated so that it can be *measured* and either enforced as a
hard gate or charged as a cost, and every rule carries its status in the current code:

| Mark | Meaning |
|---|---|
| ✅ | enforced today (hard, or a cost high enough that the fixtures never violate it) |
| ⚠ | exists only as a soft cost or heuristic; the fixtures still violate it |
| ❌ | absent |

Rule ids: **P** priorities, **G** graph preparation, **L** layering and tiers, **O** ordering,
**X** coordinates, **R** routing, **A** labels and badges, **C** containers, **V** verification.
"MUST" rules are invariants a render may not violate (an anomaly, retried, then `--strict`
fails); "SHOULD" rules are costs the search minimises in the priority order of section 1.

Numbers are in px at the default token scale (`unit` = 8 px; lane = `channel_pitch` 18 px;
port pitch `min_port_pitch` 16 px; stub `stub_clearance` 24 px; border lane 22 px).

---

## 1. Priorities (P): the order every tie is broken in

The empirical ranking (Purchase 1997/2002; Ware 2002; Huang 2010/2014; Marriott 2012;
HOLA 2016) is stable across studies, and it is *not* the order rdg's router weights encode
today. From most to least important for a reader:

| # | Property | Evidence | rdg today |
|---|---|---|---|
| P1 | **Invariants**: nothing drawn through a node, through a foreign container, or over text | all engines treat these as 1000× a crossing (yFiles node crossing 3000, port violation 10000; libavoid cluster crossing 4000) | node ✅, foreign container ⚠ (400, violated 2–8× on big fixtures), text ⚠ |
| P2 | **Crossings on the path being read**, above all crossings at a port, a bend or an arrowhead (adjacent-edge crossings) | Purchase 1997 "by far the most important"; Ware 2002: crossings on the followed path matter, total count much less; yFiles charges an adjacent-edge crossing 7× an ordinary one | crossing 100, near-bend +180 ⚠ (ride-hailing: 3 near a port) |
| P3 | **Continuity**: a route reads as one line: few bends, monotonic in the flow, no detour round the canvas | Ware 2002 (continuity second only to path length); HOLA R7/R8 (bends, long edges disliked) | bends 75 ⚠; no detour rule ❌ (8-bend perimeter loops) |
| P4 | **Label ownership**: a label is nearer its own line than any other by a wide margin, and sits where the reader looks for it (branch labels at the branch, long edges near the source) | Kakoulis & Tollis 1997 "clear graphic association"; ELK label-edge 2 px vs edge-edge 10 px; Schulze 2018 | 4 px beside ✅, foreign line ≥ 12 px soft ⚠, position policy ❌ |
| P5 | **Flow direction**: forward edges run with the flow, back edges are the exception and look like one | dot aesthetic A1; UML study 61 % prefer "arrows going down"; Marriott 2012 orthogonality/collinearity aid recall | against-flow 45 ⚠ |
| P6 | **Separation**: parallel lines a full lane apart; lines clear of node faces and borders | ELK conflict threshold 0.5 × spacing; yFiles node-to-edge 10 px, group-to-edge 10 px; libavoid shape buffer | spread pass ✅, hugging 0.5–2/px ⚠ (budget 58 hugs) |
| P7 | **Compactness, symmetry, alignment** | Purchase 1997 symmetry inconclusive; Marriott 2012 symmetry recalled; HOLA: 2–4× area still preferred when crossings/bends are fewer | aspect-targeted tiers ✅ |

Consequences for the cost model (V2): P1 and the port/bend crossings of P2 become gates, not
weights; a perimeter detour (P3) must cost more than the crossings it avoids; a label that
cannot be owned (P4) is an anomaly, not a tie-break; length (0.1/px) stays the last term.

---

## 2. Vocabulary

- **Flow frame**: `main` along the flow (y in `tb`), `cross` across it. All rules are stated in it.
- **Tier / band**: a row of units across the flow. **Channel**: the gap between two bands.
  **Gutter**: the gap between two units of one band. **Track**: a horizontal run inside a
  channel. **Lane**: a vertical run inside a gutter. Tracks and lanes are a pitch apart.
- **Stub**: the first and last straight leg of a route, ≥ `stub_clearance` long.
- **Foreign container**: a group neither end of the edge belongs to (at any nesting level).
- **Tier span** of an edge: the bands from its source's band to its target's band inclusive,
  extended across the flow to the outer edges of the units in those bands. A leg outside the
  tier span is a **perimeter leg**.
- **Hub**: a node with ≥ 5 incident edges, or ≥ 3 on one face.
- **Collector**: a node whose incident edges come from ≥ 3 different top-level groups and
  carry the same label or style (observability, auth, config).
- **Sink unit**: a node or group with inbound edges only; **source unit**: outbound only.
- **Back edge**: an edge reversed by cycle breaking; it is drawn against the flow.
- **Tight loop**: a pair A→B, B→A.

---

## 3. Graph preparation (G)

**G1 Scale budget (MUST warn).** Let N = nodes, E = edges, D = E/N.
- N ≤ 25 and D ≤ 1.5: one page. 25 < N ≤ 40 or 1.5 < D ≤ 2: `--check` and the render emit a
  warning naming the hubs (G6) and collectors (G7) as the things to split or collapse.
  N > 40 or D > 2 (and the graph is not a dense block): the warning says the diagram will not
  read as a design and should be split into an overview (groups as nodes) plus one diagram
  per group. The render still runs.
- Source: the guide's own 8–25 / split above 35; Mermaid C4 guidance; arc42 D-7.
- Status ✅ (`validate.rs`): `--check` and every render warn above 25 nodes or 1.5 edges
  per node, and more strongly above 40 or 2.0, naming the hubs (degree ≥ 5) and the
  collectors (fed from ≥ 3 groups). Ride-hailing: "38 nodes and 48 edges … hubs: gateway
  (8 arrows), payments (7), trip_svc (7), flink (5); collectors: feature_store".

**G2 Cycle breaking.** Depth-first search in declaration order; the back edge is the one that
returns to a node still on the stack. ✅ (`rdg-graph::break_cycles`). Add: a reversed edge
whose label reads as a forward step (not retry/again/back/loop/< n) is reported with its
ends so the author can reorder. ⚠ (only a generic "cycles were broken" line).

**G3 Tight loop (A→B and B→A).** Drawn as two legs one lane apart, each entering its target
at its own port, labels on the *outer* side of each (never between the two), arrowheads at
opposite ends of the pair. Never two stacked dashed lines (ride-hailing: trip-events ↔
stream-processor). Source: ELK label rule 3 (Schulze 2018); C4 "split bidirectional lines".
Status ⚠ (spread separates them; no pairing, labels may fall between).

**G4 Multi-edges (same pair, same direction).** Fan out side by side at multiples of the
port pitch, ordered, labels stacked outward from the pair; never merged. Source: dot
(displacement = multiples of nodesep); Mermaid warns they overlap otherwise. Status ⚠.

**G5 Self-loops.** Loop on the node's least populated face, extent = stub + lane, label
outside the apex; nested for several. Source: dot §5.1.3; ELK NORTH/STACKED; yFiles cost 9
for a loop leaving and entering different faces. Status ⚠ (drawn by the legacy fallback).

**G6 Hubs.** A hub's edges are ordered by the cross-flow position of their other end before
ports are assigned (O2); when demand on one face exceeds its capacity `(0.7·len / 16) − 1`
the overflow spills to the adjacent face in cross order; when ≥ 4 in-edges of one style
arrive from one band, they join a **trunk** one lane above the face (junction dots, one
arrowhead) instead of four parallel arrowheads 16 px apart. Source: ELK `mergeEdges`/
hyperedge junctions, yFiles bus routing, Bach 2017 (bundle only by connectivity, never by
proximity). Status: capacity/usage penalties ✅, ordering ⚠, trunks ✅ for both directions when the
other ends share a group (`trunk.rs`: a fan-in bundles into a trunk that keeps the
arrowhead, a fan-out branches off a trunk at the group's border, every branch starting
at a junction dot via `EdgeRoutingPlan::start`). Measured on the fan-outs the user
pointed at: ride-hailing's gateway (four `gRPC` arrows into Core platform services) and
fintech's channel BFF now run as one trunk each; ride-hailing crossings 52 → 46, totals
249 → 243. Still a ribbon: same-meaning arrows to targets that share *no* group (then
nothing marks where the trunk should split), and a hub's different-meaning arrows,
which rightly stay separate and are only spread a lane apart.

**G7 Collectors.** A collector's N edges with identical label from ≥ 3 members of one group
collapse into one edge from that group's border to the collector, labelled once; the
collector unit sits at the *side* of the tier span of its feeders (a side column), not in
the bottom tier. Source: HOLA R1 (trees and leaves outside the core), ELK
`highDegreeNodes.treatment`, the guide's own "cross-cutting concerns at the edge".
Status ⚠ half done. Bundling ✅ (`trunk.rs`, runs last on the finished routes): three or
more arrows of one meaning (same label, style, line, colour, head; never numbered ones)
from members of one group to one node outside it join a trunk — the shortest member
route leaving by the majority face; each other member is cut at the group's border, drops
a clearance plus one lane per rung past it and runs across to the trunk's line, ending in
a junction dot (SVG circle, draw.io `endArrow=oval` with a `targetPoint`), members nearer
the trunk joining first so no rung crosses another's drop; a rung that would clip a node
leaves that member alone. The plan carries the end as `EdgeRoutingPlan::junction`, which
the path helpers, the canvas and frame shifts, pixel snapping and the own-node check all
honour. Measured over the fixtures: total crossings 262 → 253, marketplace 25 → 18.
Placing the collector unit at the side of its feeders' tier span is not done (it is the
L1 side-column case, which the score settles per diagram).

**G8 Disconnected components.** Laid out separately, packed, 20 px apart, never interleaved.
✅ (fCoSE path).

**G9 Repeated labels.** Identical text on ≥ 3 edges leaving one node, or entering one node,
is drawn once at the shared end (fan label), and the other copies are dropped; identical
protocol labels on *every* edge of a diagram are reported by `--check` ("say it in the
legend or the node"). Status ✅ (`annotate.rs`: the label stays on the arrow in the middle
of the fan by the far ends' cross-flow order, or on a bundle's trunk; the others are in
`EdgeAnnotations::folded`, which both renderers honour — draw.io would otherwise draw the
edge's value itself; `--check` warns when one label sits on ≥ 5 and at least half of the
labelled edges). Ride-hailing's five `gRPC` round the gateway are now one.

---

## 4. Layering and tiers (L)

**L1 Where units sit along the flow.**
- A unit sits one tier below the lowest of its feeders (✅). A pure source sits directly above
  the first unit it feeds (✅). Two units wired both ways share a tier (✅).
- A **sink unit fed from ≥ 3 tiers** (a stores group fed by every service tier) does not go
  to the bottom tier: it becomes a **side column** spanning its feeders' tiers, placed on the
  side that minimises crossings, so every feeder reaches it by a short horizontal run. The
  bottom tier is for sinks fed from the tier above only. Source: HOLA R1; ELK high-degree
  treatment; dot "keep edges short".
  Status ⚠ measured, kept as a scored candidate (`compound.rs`: a top-level unit wired
  to ≥ 3 tiers that only answers its feeders, under 35 % of the area, is laid out
  portrait and hung beside those tiers with a gutter of one lane per arrow; the review
  routes both this and the plain tiering and keeps the better `plan_score`;
  `RDG_SIDE=on|off` forces it). On the five fixtures where a unit qualifies the column
  won once (video-conferencing: detour gone, length −9 %, crossings +4) and lost four
  times (fintech crossings 25 → 51, youtube 5 → 17): with partners spread across a wide
  body, every arrow into the column crosses the vertical arrows between its partner and
  the side. Ride-hailing's ribbon is untouched because its stores are fed by *one*
  top-level unit (Core platform services, four sub-groups), which the unit-level rule
  cannot see into. Next: decide the side and the column's position by counted crossings
  against the routed body, not by the partners' mean, and apply the rule inside a parent
  group as well as at the top level.
- A cross-cutting/collector group (G7) is also a side column. ❌
- A group wired both ways with the tier it feeds and nothing above sits beside that tier ✅
  (round 9).

**L2 Long edges (span ≥ 2 tiers) get a reserved corridor.** For each intermediate band the
edge gets a dummy item (a zero-width column) that takes part in ordering (O1) and widens the
gutter it lands in by one lane; its route is then a straight drop through those gutters with
at most two bends, never a perimeter loop. Source: dot virtual nodes with straightening
weight 8 on virtual–virtual segments ("spaghetti effect" without it); ELK long-edge dummies.
Status ⚠ partly done (`compound.rs`): inside a group every edge spanning ≥ 2 layers is a
chain of zero-width dummies that take part in the median ordering (`order_with_corridors`)
and are re-placed by the settle pass next to what they line up with; at the top level a
long edge gets a corridor slot in every tier it passes, placed by its chain neighbours;
in both the gutter the corridor sits in is the one widened (`corridor_x`). Measured: three
fixtures changed, marketplace's worst route 6 → 3 bends, adtech −4 bends, game backend +1
crossing, totals flat. **Pass-through corridors** are in too (`open_pass_through`, `LayoutResult::corridors`,
`Grid::reserved_v/h`): when a long edge's corridor falls well inside a group of the tier
it passes, that group gets a request at that cross coordinate, places a corridor at it in
every layer, then opens one straight band through all its layers (two clearances plus two
lanes wide, at the rightmost of the layers' gutter bounds, past the title), moving the
items beyond it outward where a layer's gutter is too narrow; units beside a group that
grew move over. The band is handed to the router as a reserved line: a grid line in its
own right, open to foreign edges, and crossing the group's border along it costs what an
own border does. Like side columns it is a bet settled per diagram on the routed result
(`LayoutConfig::pass_through`, `RDG_PASS=on|off`, variants judged on the first review
pass by `plan_score` + 500 per anomaly, the choice kept for the wider retries). Measured
over the 45 fixtures against the step before: total crossings 284 → 262; game backend
41 → 19 (length −16 %), fintech 25 → 21, lakehouse 10 → 9, adtech 9 → 6; healthcare
18 → 21 and ride-hailing 50 → 52 where the score preferred the variant for its other
terms. A group that holds only sub-groups was never settled before this round (its
settle pass returned on an empty node list), which also kept it from opening a lane.
Still open: the tier swap step counts no unit-versus-corridor crossings (only slot
placement sees corridors), and a lane is one straight band per edge, never shared.

**L3 Fold policy.** A strip (≥ 4 tiers, aspect > 2.2) may fold into columns only if
- the number of continuation edges (bottom of column k → top of column k+1) is ≤ 2, and
- they are routed **up the gutter between the two columns**, which is reserved for them
  (width = continuations × lane + 2 × border lane), entering the column head from the top;
  never around the far side of the canvas; and
- labels/badges of a continuation edge sit on its vertical run in that gutter, not on the
  top track.
Otherwise do not fold: a tall page beats three arrows looping round the whole drawing.
Alternative when the author prefers a page: paired on-page connectors (ISO 5807) at the
fold. Source: ISO 5807 connectors; HOLA (compactness is weak against crossings/bends).
Status ✅ (`fold.rs`, `compound.rs`: a strip folds only when no column boundary carries
more than three continuation arrows (`FOLD_MAX_CONTINUATIONS`); each continuation gets a
lane hint at the centre of the gutter between its columns (`LayoutResult::lane_hints`),
which the router makes a grid line and charges 0.6/px for running its long leg a lane or
more away from (`W_OFF_LANE`). Measured: document-approval 3 → 1 crossings and 22 → 13
bends, incident-triage and loan-approval each a bend fewer; on-page connectors as the
alternative are not done.

**L4 Tier width.** A tier is ≤ 2 × its widest unit and ≤ √(2.5 × total area) ✅. Add: a wide
tier directly under a narrow one is re-split so that no edge between them runs further
across the flow than 3 × the wider tier's tallest unit (else L1 side column). ❌

**L5 Cross-flow placement of units.** Units slide toward their connections (4 sweeps ✅),
with labelled edges and multi-edges weighted 2 and edges to hubs weighted 1/degree. ⚠
(unweighted).

**L6 Gap sizing.** Channel height = (deepest pile of turning edges + 1) × lane + labels
crossing it along the flow; gutter width = lanes passing through + 1, × lane, + labels
beside a vertical leg. ✅ (`gaps_for_tracks`, `label_across`, `label_along`). Add the trunk
junction row for hubs (G6) ❌. Source: ELK routing width = (slots + 1) × edge spacing.

**L7 No voids.** Inside a group the content box is at most 1.8 × the sum of its item boxes
(area fill ≥ 55 %); a group that fails is re-packed (inner group under the title row, nodes
in a grid beside it) before anything is routed. A void is where foreign legs and labels end
up and read as members. Status ❌ (ride-hailing "Core platform services" left half; fintech
"Lending" right half, which then hosts `gRPC`/`post` of foreign edges).

---

## 5. Ordering (O)

**O1 Crossing minimisation** over real nodes *and* long-edge dummies (L2): weighted median
sweep up to 24 iterations keeping the best count, then transpose, ties broken alternately.
Source: dot §3; ELK layer sweep + two-sided greedy switch. Status: median × 3 + transpose
✅ for real nodes, no dummies ❌.

**O2 Port order = neighbour order.** On every face the ports are in the cross-flow order of
the other ends, so no two edges of one node cross within a stub of it. Source: ELK FREE port
ordering by crossing minimisation; yFiles adjacent-edge crossing 7. Status: channel router
✅ (`assign_ports`), grid search ⚠ (only `W_PORT_TAKEN`).

**O3 Group members contiguous** ✅ (by construction). **O4 Same-tier edges run with the
reading direction** ✅.

---

## 6. Coordinates (X)

**X1 Chains straight**: a node with one in and one out edge sits on the line between them;
long-edge corridors are straight. Source: dot priority method (weights 1/2/8); ELK
Brandes–Köpf with the 2020 erratum. Status ⚠ (`straighten_layers`, a lighter heuristic).

**X2 Parent centred over the span of its children** ✅ heuristic.

**X3 Grid**: node centres on the 8 px unit grid, route legs on the lane grid of their
channel/gutter, whole pixels after polish ✅ (`spread::snap`, `snap_to_pixels`).

**X4 Entry at the face centre** unless another port is there or a bend is saved ✅
(`W_OFF_CENTRE`).

---

## 7. Routing (R)

Invariants:

**R1 Never through a node** (MUST) ✅ — obstacle in the search and an anomaly.

**R2 Never through a foreign container** (MUST). The interior of a foreign container is an
obstacle, with one exception: a gutter of it that is a *public corridor* (an edge-free gutter
that reaches from its top border to its bottom border) may be crossed straight through, at
most once, perpendicular to the borders, and costs 160. A lane the layout reserved for a
passing edge (L2 pass-through) is such a corridor by construction and costs no more than an
own border. Crossing a border of an *own*
container happens exactly once per nesting level, perpendicular to it. Source: yFiles
group-node crossing 60 vs edge crossing 1; libavoid cluster crossing 4000; Mermaid treats a
title-band crossing as an error. Status ✅ (`ortho.rs`: a foreign interior is an obstacle
to the search except along its public corridors — grid lines a border lane inside the
box and clear of every member box plus clearance and every inner box plus a border lane;
a relaxed search that only charges for it runs when no route exists, and on the 45
fixtures it never had to). Fintech went from 8 foreign crossings to 0 (crossings 23 → 25,
bends 68 → 77), ride-hailing 4 → 2 and marketplace 2 → 2, the remaining ones being
corridor crossings. Before: `W_FOREIGN_BORDER` 400 was a cost only.

**R3 Clearance** (MUST for runs ≥ 48 px). A leg runs ≥ 24 px (stub clearance) from any node
face it does not end on and ≥ 22 px (border lane) from any container border, unless it is
crossing it. Shorter runs may approach to 12 px. Source: yFiles node-to-edge 10 and
group-to-edge 10 with cost 10 each; ELK edge-node 10. Status ⚠ (0.5/px and 2/px costs;
`border_hugs` budget 58; fintech `PaymentMissed` rides the Core-domains bottom border,
game-backend three lines ride the Platform-services bottom).

**R4 Lane pitch** (MUST). Two legs of different edges that run side by side for ≥ 12 px are
≥ 18 px apart (one lane); for ≥ 160 px, ≥ 27 px. `close_parallels` must be 0 after spread;
if not, the gutter or channel they share is widened by one lane and the layout retried.
Source: ELK conflict threshold 0.5 × edge spacing; libavoid separation constraints.
Status ⚠ (spread ✅, 1–5 close pairs remain on the big fixtures, no retry).

**R5 Stubs and arrowheads** (MUST). First and last legs are straight and ≥ 24 px; no
crossing within 20 px of any port, bend or arrowhead of either route (`crossings_near_bend`
= 0); no bend within 10 px of an arrowhead. Source: yFiles min last segment 10, cost 12,
adjacent-edge crossing 7; Wybrow 2010 (crossings only at the end of a shared run, never at
a connector end); Mermaid flags a bend in the last 10 px. Status ⚠ (`W_CROSS_NEAR_BEND`
180; ride-hailing 3, social 1, hybrid 1 remain).

**R6 Bends.** A route has 0, 2 or 4 bends by structure; 3 only for an L to a side face; a
5th bend is allowed only to clear an obstacle the layout could not move (a title band),
and > 5 is an anomaly that triggers a layout retry, never a drawn result. Source: channel
routing (ELK: bends only where direction changes); Purchase: bends second to crossings.
Status ⚠ (`review.rs`: a searched route with > 5 bends is a `TangledRoute` advisory,
reported on stderr and in `max_bends=` but, since wider spacing never removed one on the
fixtures and chased a worse layout on adtech, it does not drive the retry nor fail
`--strict` until L1/L2 give the retry something to do; fintech still has an 8-bend route,
incident-triage a 7-bend one).

**R7 Monotonic flow** (SHOULD, cost ≥ a crossing). A forward edge has no leg running against
the flow except a final approach ≤ 24 px into a side face. A back edge has exactly one
against-flow run. Source: yFiles monotonic path restriction (cost 14); dot A1.
Status ⚠ (`W_AGAINST_FLOW` 45 < one crossing).

**R8 No perimeter detours** (MUST). A route stays inside its tier span, extended by one lane
on each side; it may leave it only when it is a back edge or a fold continuation (L3), and
then by the side nearest both its ends, at most one lane beyond the outermost unit of the
tiers it passes. Detour ratio = route length ÷ Manhattan distance of its ports ≤ 2.5;
above that the route is an anomaly (a layout problem: L1/L2/L7) and the layout is retried,
not the route. Source: HOLA R8 (long edges disliked), dot A3 (short edges), Bach 2017
(a ribbon of parallel lines cannot be followed). Status ⚠ (`review.rs`: ratio > 2.5 with
excess > 160 px is a `Detour` advisory, counted in `detours=` and charged 200 per route in
the router-choice score; advisory for the same reason as R6 — the fix is L1/L2/L7, not
spacing. Three fixtures carry one each: adtech `openrtb_gw → ssp`, video-conferencing
`transcoder → recordings`, and ride-hailing's margin ribbon reads as 0 detours because
each line's ports really are that far apart, which is the L1 sink-column case).

**R9 Ports and faces.** Forward edge: leaves the face toward the target in the flow frame
(bottom for a lower tier, side for the same tier), enters the top face; the port along the
face follows O2; icon nodes are never entered under their caption ✅ (`W_UNDER_LABEL`). A
face holds at most `capacity` ports 16 px apart; past that, spill to the adjacent face in
order ✅; a trunk when G6 applies ❌. Decisions take arrows only at their corners ✅; the
`yes`/main branch continues down, the exception branch leaves a side corner (flowchart
convention) ⚠ (not enforced).

**R10 Tight loops**: see G3. **R11 Back edges**: leave the source's side face nearest the
target column, run up the nearest gutter, enter the target's side face; they enter a *top*
face only when the target is a column head (fold). Source: yFiles back-loop routing; ELK
feedback edges "routed around the nodes" nearest the target. Status: channel router ✅
(`Kind::Up`), grid search ⚠ (social-graph loops over the top row).

**R12 Shared corridors keep one order**: edges sharing a track or lane are ordered by where
they leave the corridor, so a crossing occurs only at the end of the shared run (Wybrow
2010, Theorem 3; ELK slot dependency graph, crossing penalty 16 vs conflict 1). ✅ in both
routers (`nudge`, `pack_lanes`, `assign_tracks`).

**R17 Sibling forks turn on one line.** Two arrows that leave one face of a node (or
arrive at one) along its normal and then turn *opposite* ways turn at the same depth,
the way a person draws a fork: one bar across, branches off it. Arrows turning the
*same* way keep their own lanes (R4). Only turns within two lanes of each other count;
further apart they are in different channels for a reason. The nearer turn moves out
to the farther one's line, never inward, never past its next bend (a clearance before
the leg beyond), never onto a node, never within a lane of another arrow's parallel
leg, and never into more crossings than it had. Source: HOLA (aligned edge segments
read as one structure), Purchase 2002 (orthogonality and alignment). Status ✅
(`siblings.rs`, run after `spread` in `routing.rs`; `siblings=u/p` on the spacing
line). Measured: 26 of 49 same-channel sibling pairs aligned, every fixture's crossings,
bends and length unchanged (totals 243 → 243); the 23 left are blocked by one of the
guards. An unbounded version that moved to any partner's depth moved turns hundreds of
px and cost 7 crossings, hence the two-lane bound.

**R13 Crossings are 90°** by construction ✅; **crossings per route ≤ 3**, else the route is
an anomaly ❌ (not measured per route).

**R14 Line jumps are the last resort.** When a diagram still has ≥ 10 crossings after
layout and routing, horizontal legs jump (arc, width 10, height 5) over vertical ones,
always in that direction, in both renderers. Source: yFiles BridgeManager (horizontal over
vertical), draw.io `jumpStyle`. Status ✅ (`routing::JUMPS_FROM_CROSSINGS` = 10; SVG:
`path.rs` hops a horizontal leg over every crossing vertical one with a 5 px half
circle, sweeping with the direction of travel; draw.io: `jumpStyle=arc` on every edge
under the same rule or the theme's `edge.line_jumps`). Ride-hailing draws 47 hops.

**R15 Determinism** ✅ (declaration order everywhere; no hash-map iteration reaches output).

**R16 Router composition.** The structural router (channels) plans every edge it can; the
grid search plans only the rest *with the structural plan's ports and lanes reserved*; the
choice is per edge, not per diagram. Status ✅ as a third candidate (`channels::
route_channels_partial` keeps the plans that stood and counts the rest;
`ortho::route_orthogonal_with` seeds the search's occupancy with those plans' cells, bends
and ports and routes only the rest; `plan_all_edge_routes` scores channels, composite and
search and keeps the best, `RDG_ROUTER=composite` forces it). Measured: the channel router
places 48 of 48 ride-hailing edges but one failed check used to send the whole diagram to
the search; across the 54 router choices on the fixtures the composite now wins 3,
channels 5, search 46 — the search still scores best on most big diagrams because its
plans have fewer bends and shorter length, which the score weighs after crossings.

---

## 8. Labels and badges (A)

**A1 Ownership** (MUST). Let g be the gap between the label box and its own line (4 px
beside, or 0 on the line with a knock-out). Every other line is ≥ g + 18 px (one lane
further than the own line) from the box, and the label box lies within the perpendicular
band of its own leg. A sibling arrow on the far side of the own line one port pitch away
therefore passes; a line on the label's own side within a lane does not.
Status ✅ as a cost (another line inside the band costs 8/px, fading with distance) and as
a report: `RDG_DEBUG_SPACING` prints `labels: floating=N` with one line per violation.
Still floating on the fixtures after this (14 of ~450 labels): hub fan-ins where three
arrows share one face (fintech `post entry`), and parallel pairs with no room on either
side; those are G6/R4 work, not label work. A label
that cannot satisfy this anywhere on its edge is an anomaly (A7 applies). Source: ELK
label-edge 2 vs edge-edge 10 (≥ 1:5), Schulze 2018 ("noticeably smaller"); Kakoulis &
Tollis "a label that floats between edges". Before this round: `W_NEAR_LINE` 4/px over a flat 12 px band, so a label 4 px from one
line and 14 px from the next cost nothing extra.

**A2 No overlap** (MUST). The label box, padded 2 px, overlaps no node, title, border, other
label, badge or foreign line. Status ⚠ (costs 60/px², 80/px, 30/px; healthcare `ResultReady`
sits on the Stores border).

**A3 Position along the edge** (the rule that fixes the floating `gRPC`s). Candidates in
order; the first that satisfies A1–A2 wins, later ones only break ties:
1. **Branch labels** (edge leaving a decision/choice, or a state transition): on the first
   leg, within 24 px of the exit corner, on the outer side of the branch. Source: BPMN
   Method & Style, UML guards, ELK TAIL_LAYER for transitions.
2. **Tail position**: on the first straight leg after the stub, within one node height of
   the source; this is where a reader following the arrow starts.
3. **Longest straight leg inside the tier span** (section 2), nearer the source on ties.
4. **Head position**: the last leg, before the arrowhead clearance.
Never on a perimeter leg (R8), never inside a foreign container, never on a leg that is a
shared corridor with ≥ 2 other edges. Source: Kakoulis & Tollis preference "closer to the
source"; Schulze 2018 (median-layer labels make users "search a large area"); Graphviz
head/tail labels. Status ✅ (`annotate.rs`: 0.15/px drift from the source, branch labels within 40 px of the
corner or of the end of the stub the branches share, +600 on a leg outside the two nodes'
box, +300 inside a foreign container). Before: longest leg anywhere + 0.03/px drift, which
is why `gRPC`/`verify`/`report`/`sessions` sat 500–1000 px from both ends. Repeated
labels now cluster at their shared source (five `gRPC` round the gateway): G9 is next.

**A4 Side.** One consistent side per orientation (above a horizontal leg, right of a
vertical one) ✅; for a tight loop or two parallel legs, the outer sides, never between ✅
(the ownership cost fades with distance, so the side further from the other line wins;
unit test `a_label_between_two_parallel_arrows_moves_to_the_outer_side`); the outermost
edge of a run puts its label outside the run ✅ (same mechanism). Source: ELK
side-selection rules 1–3.

**A5 Short edges.** Edge shorter than label + 2 stubs: the label goes on the line (knock-out,
one line of text) ✅, or the gap between the ranks is widened for it at layout time ✅ (`lr`
only — `tb` relies on the label standing beside the vertical leg, which needs gutter width:
✅ `label_across`).

**A6 Knock-out.** Every label and badge has a plate in the canvas colour, 2 px pad ✅ both
renderers. Source: Schulze 2018 (on-edge labels with background rank highest); Imhof halos.

**A7 Leader line.** When no candidate within 40 px of the edge satisfies A1–A2, the label is
placed in the nearest whitespace and a 1 px leader joins it to its leg's midpoint. Source:
Graphviz `decorate=true`. Status ✅ (`annotate.rs`: candidate gaps of 16, 28 and 40 px at
rising prices, `LabelSpot::leader` from 16 px; a leader label's ownership band is one lane,
since the leader is the ownership; SVG draws a 0.8 px line from the anchor to the nearest
point of the box, draw.io a thin edge cell `<edge>_leader` between those points). Thirteen
labels across the fixtures take a leader; floating labels went from 12 to 11.

**A8 Repeated labels**: see G9 ❌.

**A9 Badges.** Beside the line, never on it, within 96 px of the source (any point past the
arrowhead on an arrow under 140 px), ≥ 10 px from any crossing ✅. On a continuation or back
edge longer than 600 px a second, smaller badge sits before the arrowhead ❌.

**A10 Labels are reserved in layout.** A labelled edge widens the gutter/channel it uses by
its label's extent ✅ partial (L6); a dummy that takes part in ordering ❌ (L2).

**A11 Distance from bends and heads.** ≥ 10 px from a bend ✅ (was 2 px), ≥ 14 px from
the arrowhead ✅.

**A12 Wrapping**: ≤ 22 characters per line, ≤ 2 lines; on-line labels are one line ✅.

**A13 Colour is never the only cue**: a dashed orange label beside a dashed orange line is
still subject to A1 ✅ (A1 applies to all).

---

## 9. Containers (C)

**C1 Padding and title**: 32 px sides, title band is an obstacle and no arrow passes through
it ✅. **C2 Border crossings**: once per level, perpendicular (R2/R3) ⚠. **C3 Group-level
edges**: ≥ 3 members of a group to one outside node collapse to a border edge (G7) ❌.
**C4 Inner group placement**: an inner group is one item of its parent's layers and the
parent's remaining items fill around it; L7 bounds the void ❌. **C5 Title clear of arrows**
✅. **C6 Foreign legs and labels never inside a container** (R2, A3) ⚠.

---

## 10. Verification (V)

**V1 Gates** (anomaly → retry with wider spacing → `--strict` fails). Today: node overlap,
edge through node, label overflow, extreme aspect, congested corridor, overlapping paths,
excessive bend (non-searched routes only), through own node. **Advisories** (reported,
not retried, not failed on, because only a layout change can clear them): `Detour` (R8)
and `TangledRoute` (R6) ✅. Still to add as gates: hug ≥ 48 px (R3), close pair (R4),
crossing within 20 px of a port/bend (R5), label ownership (A1), label overlap (A2) ❌;
foreign crossing (R2) is now prevented at the source instead.

**V2 Score** (used to pick between candidate plans). Re-weight to section 1: gates first;
then crossings 100, adjacent/near-port crossings 300 (not 70 — they are the worst kind, not
a milder one), perimeter detour 200 per route, bends 40, against-flow legs 50, border hugs
40 per 48 px, off-centre 20, close pairs 100, length 0.1/px. ⚠

**V3 Per-diagram ceilings** (relative to size, reported in `--format json`): crossings ≤
0.5 E, bends ≤ 2 E, max bends per route ≤ 5, close = 0, foreign = 0, near_bend = 0, label
ownership violations = 0, routes with detour ratio > 2.5 = 0. The fixed fixture budgets in
`crates/rdg-cli/tests/cli.rs` (crossings 300, near-bend 14, hugs 58 over 49 fixtures) stay
as regression guards. ✅ (`report.rs`: a `quality` object in the JSON with every measure,
its ceiling and the `exceeded` list; in text mode one `ℹ quality:` line when any ceiling
is exceeded. Fintech today: crossings 21 of 18 allowed, max bends 6, 3 close pairs,
2 foreign crossings, 2 floating labels.)

**V4 `--check` scale report** (G1, G6, G7, G9): node and edge counts against the budget,
the hubs by degree, the collectors, repeated labels. ✅ (G1's warning plus the
repeated-label warning; both on `--check` and on every render).

---

## 11. Edge-case catalogue

Each case names the symptom seen in a bundled fixture (rendered at 6ccd100), the rules that
cover it and the current status.

| # | Case | Symptom (fixture) | Rules | Status |
|---|---|---|---|---|
| 1 | Fold continuation edges | 3 arrows loop round the far side of the canvas and converge on one top face (loan_approval 8/9/10; incident_triage page_lead→bridge, attach→close, ticket→close) | L3, R8, R11, A9 | ⚠ |
| 2 | Sink group fed from ≥ 3 tiers | stores at bottom right; five 1500 px verticals on the right margin (ride_hailing); `ledger`/`customers` (fintech) | L1 side column, R8, G6 | ❌ (layout); routes no longer cut containers on the way) |
| 3 | Collector node (observability, ML, auth) | `traces` ×3, `metrics`, `score`, `features` from everywhere to the bottom tier (ride_hailing, marketplace) | G7, L1 | ❌ |
| 4 | Long edge spanning ≥ 2 tiers | routed round the outside with 6–8 bends (`idp → customer_db`, fintech `channel BFF → accounts`) | L2, R6, R8 | ⚠ reported (`TangledRoute`/`Detour` advisories); layout fix pending |
| 5 | Back edge | loops over the top row and enters a top face (social_graph; incident_triage `escalate → ack`) | R11, R7 | ⚠ |
| 6 | Tight loop A↔B | two stacked dashed lines, heads at both ends (ride_hailing trip-events ↔ stream-processor / trip-service) | G3, A4 | ⚠ |
| 7 | Multi-edge same direction | — (not in fixtures; would overlap) | G4 | ⚠ |
| 8 | Self-loop | learning_platform `categories → categories` 13 px from its neighbour | G5, R4 | ⚠ |
| 9 | Hub fan-in beyond face capacity | ledger-service: 3 heads within 40 px on the right face + top + left (fintech); `trips`/`ehr` 4–6 heads | G6, O2, R9 | ⚠ (trunk ❌) |
| 10 | Identical labels near each other | `gRPC` ×5, `HTTPS` ×4, `WebSocket` ×3, `traces` ×3, `conform` ×2 | G9, A8 | ❌ |
| 11 | Label on a perimeter leg | `gRPC`, `verify`, `report`, `sessions`, `features`, `parquet`, `SIP`, `results`, `match events`, `write back` 500–1000 px from both ends | A3, R8 | ✅ labels (routes still loop: R8 ❌) |
| 12 | Label between parallel lines | right-margin ribbon (ride_hailing): label 4 px from one line, 14 px from the next | A1, A4, R4 | ✅ labels, ⚠ where no side has room |
| 13 | Label on a short edge | handled: on-line or gap widened | A5 | ✅ |
| 14 | Label at a bend | 2 px inside the segment was allowed | A11 | ✅ (10 px) |
| 15 | Label at a crossing | soft cost only | A2 | ⚠ |
| 16 | Label over a container border | `ResultReady`/`reports` near the Stores border (healthcare) | A2, C6 | ⚠ |
| 17 | Label (or foreign leg) in a container void | `gRPC`, `post` inside the empty half of Lending (fintech) | L7, R2, A3 | ✅ legs and labels kept out (R2, A3); the void itself remains (L7 ❌) |
| 18 | Decision branch label away from the diamond | `no` placed after the bend 100 px below Known issue?; `yes` on the far side of a loop (incident_triage) | A3.1, R9 | ✅ (within 40 px of the corner, or of the shared stub's end) |
| 19 | Decision with ≥ 4 branches | — | R9, guide (chain decisions) | ⚠ |
| 20 | Edge must leave a container by the side away from its target | gateway edges out of Edge group sideways then round (marketplace API Gateway → catalog) | R7, `W_BACK_EXIT` | ⚠ |
| 21 | Edge between two sub-groups inside one parent | runs cut between Payments and Lending inside Core domains (fintech) and read as members | R2 (own container), L7, R3 | ✅ sibling sub-groups are foreign and now obstacles |
| 22 | Same-tier groups wired both ways | Commerce ↔ Fulfilment: long horizontal runs at several y (marketplace) | O4, R12, L5 | ⚠ |
| 23 | Wide tier under a narrow tier | four groups in the bottom tier (ride_hailing) force 1000 px horizontal runs | L4, L1 | ❌ |
| 24 | Void inside a group | Core platform services left half empty (ride_hailing), Lending right half (fintech) | L7 | ❌ |
| 25 | Disconnected components | — | G8 | ✅ |
| 26 | Dense graph (E > 2 N) | √n block, every back edge loops outside (social_graph) | G1 warning, R11, R14 jumps | ⚠ |
| 27 | Left-to-right diagram, labels along the flow | lakehouse: fine; `write back` on a 1200 px return | A3, L3 (no lr fold ✅) | ⚠ |
| 28 | Arrowhead hidden by a crossing line | not seen, cost only | R5 | ⚠ |
| 29 | Bends of different routes stacking at one x/y | prevented by lane grid + spread | X3, R4 | ✅ |
| 30 | Entering against the flow after a loop | `Player auth → players` enters from the right after circling (game_backend) | R7, R11 | ⚠ |
| 31 | Ribbon of ≥ 3 parallel lines on a margin | ride_hailing right margin; game_backend bottom | R8, L1, G6 | ❌ |
| 32 | Diagram above the budget | all six worst fixtures (32–49 nodes) | G1, V4 | ❌ |
| 33 | Cycle broken at the wrong edge (author order) | not in fixtures; reported generically only | G2 | ⚠ |
| 34 | Router falls back wholesale | every big fixture goes to the grid search | R16 | ❌ |
| 35 | Hugging a border for a long run | `PaymentMissed` along Core-domains bottom (fintech); three lines along Platform-services bottom (game_backend) | R3 | ⚠ |
| 36 | Crossing at a port | ride_hailing 3, social 1, hybrid 1 | R5, O2 | ⚠ |

---

## 12. Where the implementation stands, and what to do first

The pipeline today: `rdg-layout` (longest-path ranks, median + transpose, chain
straightening, compound tiers, fold) → `routing::plan_all_edge_routes` (channel router or
grid search, chosen per diagram by `spacing_metrics`; then `spread`, `polish`) →
`annotate::place_edge_annotations` (labels and badges scored against the finished routes) →
`review` (anomalies, retry with `retry_spacing_factor`). The big diagrams fail because
everything that keeps a route readable is a *cost* inside a 12-term grid search, labels are
placed *after* routing with "longest leg" as their preference, and the layout never reserves
corridors for long edges, sinks or collectors — so the search is left to thread 40 edges
through a layout with no room, and does it by going round the outside.

Order of work, by reader impact per change:

1. **Label policy (A1 cost + report, A3 order, A4 pairs, A11)** — done in this round
   (`annotate.rs`, `main.rs`): routes unchanged on every fixture, labels moved to their
   sources and decision corners; 14 labels still float, all at hub fan-ins or parallel
   pairs. A1 as a hard gate (anomaly + retry) and A7 leader lines remain.
2. **Foreign interiors as obstacles, detour and tangled-route advisories (R2, R8, R6)** —
   done in this round (`ortho.rs`, `review.rs`, `routing.rs` score): foreign crossings
   gone except through public corridors; detours and > 5-bend routes reported as
   advisories and charged in the router choice. Making them retry-driving gates waits
   for step 3, which is the only thing that can clear them.
3. **Sinks and collectors as side columns, long-edge dummies (L1, L2, G7, O1)** —
   side columns done as a scored candidate (`compound.rs`, `review.rs`): wins on one of
   five fixtures where it applies; see L1 for why and what to try next. Long-edge
   corridors done between units, inside groups and straight through foreign groups
   (reserved lanes the router honours), each a scored candidate; total crossings on the
   fixtures 284 → 262. Collectors (G7) remain.
4. **Hub trunks and repeated-label folding (G6, G9, A8)** — done for collectors (G7
   bundles, fan-in and fan-out: `trunk.rs`) and fan labels (G9: `annotate.rs`,
   `validate.rs`); a trunk for same-meaning arrows whose other ends share no group
   remains.
5. **Fold continuation in the inter-column gutter, or no fold (L3)** — done (lane hints
   from `fold.rs` and `compound.rs`, `W_OFF_LANE` in `ortho.rs`, at most three
   continuations per boundary).
6. **Scale report and relative ceilings (G1, V3, V4)** — done (`validate.rs` scale
   warning with hubs and collectors, `report::Quality` in the JSON and a text note).
7. **Per-edge router composition (R16)**, **line jumps (R14)** and **leader lines (A7)** —
   done; all three are measured as modest (the composite wins 3 of 54 choices, jumps and
   leaders change no metric but what a reader can tell apart).
8. **Hub feedback round (G6 fan-out trunks, R17 sibling forks)** — done; fan-out
   trunks measured 249 → 243, sibling-fork alignment metric-neutral. Z-jogs: the
   off-centre port on the nearest face already exists (overlap ports in `routing.rs`);
   the jogs that remain come from tile marks in neighbouring columns not lining up,
   which is a layout matter (X), not a routing one.

Measure every step with `RDG_DEBUG_SPACING=1` over the 45 fixtures (`crossings`,
`near_bend`, `bends`, `zigzags`, `foreign`, `close`, `border_hugs`), plus the new per-route
detour ratio and per-label ownership margin, and look at ride_hailing, fintech, marketplace,
game_backend, healthcare and loan_approval before and after.

---

## Sources

- Purchase, "Which aesthetic has the greatest effect on human understanding?", GD 1997;
  Purchase, Carrington, Allder, JGAA 2002 (UML preferences); Purchase, Pilcher, Plimmer,
  TVCG 2012 (user sketches).
- Ware, Purchase, Colpoys, McGill, "Cognitive measurements of graph aesthetics", Information
  Visualization 2002.
- Huang, "Using eye tracking to investigate graph layout effects", 2008; Huang, Eades, Hong,
  "Larger crossing angles make graphs easier to read", JVLC 2014.
- Marriott, Purchase, Wybrow, Goncu, "Memorability of visual features in network diagrams",
  TVCG 2012.
- Kieffer, Dwyer, Marriott, Wybrow, "HOLA: human-like orthogonal network layout", TVCG 2016.
- Gansner, Koutsofios, North, Vo, "A technique for drawing directed graphs", TSE 1993;
  Graphviz attribute reference (`splines`, `concentrate`, `decorate`, `labelangle`).
- Brandes, Köpf, "Fast and simple horizontal coordinate assignment", GD 2001; erratum
  arXiv:2008.01252.
- Wybrow, Marriott, Stuckey, "Orthogonal connector routing", GD 2009/2010; libavoid
  `router.h` (nudging distance 4, penalties).
- Eclipse Layout Kernel: `org.eclipse.elk.layered` option reference; `OrthogonalRoutingGenerator`
  (conflict 1, crossing 16, threshold 0.5 × spacing); Weber, "Improved vertical segment
  routing for Sugiyama layouts", Kiel 2019.
- Schulze, Wechselberg, von Hanxleden, "Edge label placement in layered graph drawing",
  Diagrams 2018, and Kiel TR 1802 (label side rules, on-edge labels, 2 vs 10 px).
- Kakoulis, Tollis, "An algorithm for labeling edges of hierarchical drawings", GD 1997;
  Doğrusöz, Kakoulis, Madden, Tollis, "Edge labeling in the Graph Layout Toolkit", GD 1998.
- yFiles for HTML: `EdgeRouterCosts`, `EdgeRouterEdgeDescriptor`, `HierarchicalLayout`,
  `PreferredPlacementDescriptor`, `BridgeManager` documentation.
- Bach, Riche, Hurter, Marriott, Dwyer, "Towards unambiguous edge bundling", TVCG 2017.
- Niedermann et al., "A simple pipeline for orthogonal graph drawing", arXiv:2309.01671.
- draw.io connector styles and line jumps; Mermaid layout-makers guide (validator: bend 1,
  crossing 3, label off its edge rejected); PlantUML layout engines (`linetype ortho` label
  defect).
- ISO 5807:1985 flowchart symbols and conventions; Silver, BPMN Method & Style (gate
  labels); C4 model notation and review checklist (Brown).
- Imhof, "Positioning names on maps", 1975; Yoeli, "The logic of automated map lettering",
  1972.
