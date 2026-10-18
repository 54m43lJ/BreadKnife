# Documentation Guidelines

<a id="top"></a>

Documents per project, in HTML. The development docs split by boundary: upper (users, dependent software) → USER · none (internal only) → CORE · lower (data sources, dependencies) → PROVIDER.

| # | Document | Boundary | Content |
|--:|----------|----------|---------|
| 1 | [README](#readme) | — | WHAT & WHY · user interactions · prototype |
| 2 | [USER](#user) | upper — end users / dependent software | protocols · conventions · APIs · workflows |
| 3 | [CORE](#core) | none — internal only | internal data structures & APIs · call stacks |
| 4 | [PROVIDER](#provider) | lower — data sources / dependencies | protocols · conventions · APIs · workflows — only when such a boundary exists |
| 5 | BUILD.html | — | written after development: how to use & build on the project; not a design-phase document, but kept |

**End goal** — a set of design constraints immediately useful to any developer implementing the code.

**AVOID at all costs** — comparisons to historical design · rationale for a specific design choice (broad general principles excepted).

## Global constraints <a href="#top">↑</a>

| # | Constraint | Rule |
|--:|-----------|------|
| 1 | Single source of truth | a definition appears once, at the most appropriate spot; everywhere else → hyperlink |
| 2 | No amendment history | never record changes against previous versions — it only adds confusion |
| 3 | `reference/` | optional; stores official external documentation as the authoritative source of truth |
| 4 | HTML escaping | escape `&` `<` `>` as entities wherever they are content, not markup — `<pre>/<code>` especially: unescaped brackets are parsed as tags and silently deleted (`Result<T, E>`, ASCII arrows `<--`) |
| 5 | Notes | supplements under diagrams/tables/file trees/call stacks → `<ul class="note"><li>…</li></ul>` (token `--note` in style.css), never flowing prose |
| 6 | Section layout | development docs only — opening: qualifying prose (when structure is impossible) · middle: structured content only (code/diagrams/tables) · end: `ul.note` bullets only |
| 7 | Comment budget | an in-code comment with >3 information points → `ul.note` after its code block; the comment keeps only what the signature/call needs |
| 8 | One block, one flow | a code block is a single continuous content; disjoint pieces → separate blocks/subsections |
| 9 | CONVENTIONS chapter | USER / CORE / PROVIDER each keep one, right after the first chapter (may stay empty); stated once there, referenced from the affected sections |

## Readme <a href="#top">↑</a>

1. **WHAT & WHY** — IDEAL: the problem to solve · GOAL: what success looks like
2. **User interactions**
3. **Prototype/mock** (if applicable) — code, pictures or videos

## User <a href="#top">↑</a>

Everything a customer — a dependent app, an end user — directly interacts with: protocols, conventions, APIs, workflows.
Internal mechanics → [CORE](#core) · underlying data sources → [PROVIDER](#provider)

**Rules**
- finer requirements not expressible by a straightforward mechanism → complement at the end of the section; last resort — prefer mechanism over specific constraint

**Chapters**
1. **TERMS** — domain keywords, never synonymized, full definition; when used, no further explanation
2. **CONVENTIONS** — shared across interfaces (Global constraints #9)
3. **DIAGRAMS** — mechanisms are the preferred documentation form
   - general architecture — how the application interacts with others (component diagram)
   - workflows — business language; terms/API names only for well-known standards; sequence / swimlane-activity form per [Diagrams](#diagrams)
4. **INTERFACES**
   - exposed API specifications — code, or pseudocode for multi-language support
   - message types incl. errors — pseudocode definition block + table of meaning & suggested action
   - data models — objects of exposed APIs (bare code/pseudocode, no comments) · field table (external specs referenced) · transformations (exact data object names)

## Core <a href="#top">↑</a>

Internal mechanics only: data structures & APIs that only the project itself accesses, and the run logic as call-stack trees.
Usage-facing → [USER](#user) · provider boundary → [PROVIDER](#provider)

**Rules**
- not exhaustive — key execution paths only: startup, key events, shutdown
- symbols already defined in USER/PROVIDER → one-line declaration with trailing `//见 USER §xx.xx` / `//见 PROVIDER §xx.xx` — project language, no body, no redefinition; everything else syntactically valid in the project language
- no engineering constants (bus names, periods, capacities, …) — tuned against the real system, they live in code alone, trade-off → implementation notes when it matters; a literal may appear only inline in a call stack, where that flow depends on it; exception: UI visual literals (dimensions, radii, font sizes, …) pin the visual contract and may be stated in interface/style sections (e.g. USER's component style spec) — CSS/code stays the operational source of truth
- no implementation reference → draft meaningful names/signatures marked `待实现，可以根据实际情况调整` (call-stack lines use the drafted names, no marker at the call site); a reference exists (main branch, breadpan, std / GTK APIs) → use its real names

**Chapters**
1. **File tree** — final code structure; each entry states what the module serves and links to its signatures subsection (no subsection → plain entry)
2. **CONVENTIONS** — shared across interfaces (Global constraints #9)
3. **Internal types & method signatures** — one subsection per source file, in file-tree order, titled by file name, linking back to the file tree; signature + at most a one-line purpose note — internals (call chains, branching, fallbacks, ordering) belong to the call-stack section
4. **Call stack tree** — actual execution of key program logic
   - lines carry calls or judgments only — every line points at a concrete call (`module::fn(args)`, DBus method, event send, …); a judgment/branch condition is the sole exception
   - at most one trailing comment per call — a comment is not a tree node (no `├──`/`└──` of its own), but the ancestor spine `│` continues through its line
   - motivations, policies, trade-offs beyond one comment line → prose after the block

## Provider <a href="#top">↑</a>

Exists only when the project depends on underlying data sources/services (system services, IPC buses, hardware, daemons, network endpoints, …); otherwise omitted.
Everything toward those dependencies: their protocols, conventions, APIs, workflows.
Whole-system placement → USER's general architecture · internal handling → [CORE](#core)

**Rules**
- contractual = the consumed subset only; the full authoritative definition is the official documentation (mirror in `reference/` when applicable) — linked, never copied
- workflow diagrams name the provider's methods/messages crossing the boundary; the project's internal call structure never appears — that is CORE's call-stack tree

**Chapters**
1. **TERMS** — provider-specific keywords, never synonymized, full definition; when used, no further explanation
2. **CONVENTIONS** — shared across the provider boundary (Global constraints #9)
3. **DIAGRAMS** — boundary workflows; sequence / swimlane-activity form per [Diagrams](#diagrams)
4. **INTERFACES**
   - consumed API specifications — code, or pseudocode for multi-language support
   - message types sent/received on this boundary, incl. errors — pseudocode definition block + table of meaning & suggested action
   - data models — objects crossing this boundary (bare code/pseudocode, no comments) · field table (external spec as authority) · transformations between provider ↔ project data objects (exact names)

## Diagrams: PlantUML <a href="#top">↑</a>

<a id="diagrams"></a>

Applies to USER & PROVIDER. Mermaid is banned outright; ASCII graphs / actual pictures for everything else.
Sources (`*.puml`) and exports (`*.svg`) live in the owning project's own `diagram/` — the folder follows its project, never a repo-wide root; the SVG is embedded via `<img>` with a relative path (`diagram/…`).

1. **Form by complexity** — general architecture: component diagram · at most one branch decision (<2 nesting levels): sequence diagram · more branching: activity diagram, one swimlane per participant
2. **Vertical layout** — branch-heavy flows as guard-clause cascades: checks run sequentially, each branch ends `stop` — never nested `if/else`, which PlantUML spreads side-by-side
3. **Labels** — in the documentation language; short messages, details into notes
4. **Source is authoritative** — edit `*.puml`, re-export (`plantuml -tsvg <project>/diagram/*.puml`), commit both together; never hand-edit the exported image, never let source and image diverge
5. **Embedding** — wrap in `<div class="diagram">`, natural size, horizontal scroll when wider than the page (same mode as `<pre>`); never scale down
