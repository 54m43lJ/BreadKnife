# Documentation Guidelines
Each project should contain the following documents in HTML:
1. README.html
2. ARCHITECTURE.html
3. CODE.html
4. BUILD.html — produced once development is complete: it guides other developers in using and building on
   the project; it is not written during the design phase, but it is a document the project keeps.

The end goal is a comprehensive set of design constraints that is immediately useful to any developer implementing the code. 

The following content should be AVOIDED at all costs:
- Comparison to historical design.
- Reasoning for a specific design choice (except general design principles that applies to a broad scope).

-----

Other constraints:
- Optinally, `reference/` directory can store any official documentation downloaded from internet as a authoritative source of truth.
- Adhere to the single source of truth rule. Any definition should only appear once across the entire project in the most appropriate position, and other appearances should always come in the form of hyperlinks.
- Do not keep any record of amendments to previous versions because it only add confusion to the context.
- Escape `&`, `<` and `>` as `&amp;`, `&lt;` and `&gt;` wherever they are content rather than markup — code blocks (`<pre>/<code>`) especially. Browsers parse unescaped angle brackets as tags and silently delete them (generics like `Result<T, E>` lose everything from `<` on; ASCII-diagram arrows like `<--` vanish too). Only real HTML tags stay unescaped.
- Supplementary text (notes under diagrams, tables, file trees and call-stack blocks) is always a bulleted list in a darker gray: `<ul class="note"><li>…</li></ul>` (color token `--note` in style.css) — never flowing prose.
- Section layout only applies to the development documents — ARCHITECTURE and CODE: the opening may carry paragraph-form qualifying text (when structured representation is not possible); the middle must be structured content — code blocks/diagrams/tables — never prose; the end may carry bulleted-point supplements (`ul.note`) only.
- An in-code comment carrying more than three points of information becomes a trailing supplement list (`ul.note`) after its code block; the comment itself keeps only what the signature or the call needs.
- A code block must contain a single continuous content. Never pack disjoint pieces (e.g., two unrelated flows) into one code block — split them into separate code blocks or subsections.
- ARCHITECTURE and CODE each keep a dedicated chapter for conventions shared across interfaces — placed right after the first chapter; it may stay empty. A convention is stated once there and referenced from the affected sections instead of being repeated.

## Readme
1. WHAT & WHY
	1. IDEAL: the problem we aim to solve
	2. GOAL: what success looks like
2. define user interactions
3. (if applicable) prototype/mock, can be code, can be pictures/videos
## Architecture
RULES:
- **mermaid graph is to be completely avoided. Architecture diagrams, sequence diagrams and activity diagrams are drawn with PlantUML (see "Diagrams: PlantUML" below); ASCII graph or actual pictures for everything else.**
- **Architecture document should be customer-facing. Everything a customer (app that depends on it, user of this application, etc.) would directly interact with should be documented here. The remaining internal structure and mechanisms should be in CODE.html instead.**
- Optionally, any finer requirement that cannot be described by straightforward mechanisms should be attached to the end of that section as complement. This part should always be the last resort, prefer mechanism over specific constraint whenever possible.

-----

1. TERMS: a table of doamin-specific keywords that should never be synonymized, with their full definition. When these terms are used, no further explaination of the term is allowed.
2. CONVENTIONS: shared conventions across interfaces — dedicated chapter right after the first chapter (see "Other constraints"); may stay empty.
3. DIAGRAMS: document all relevant architecture as mechanisms, which is the preferred method.
   1. General architecture (how the application interact with others)
   2. Sequence diagrams (how the program logic flows, in business language only, minimal amount of terms and API usage is allowed when referring to well-known standards)
4. INTERFACES:
   1. all exposed API specifications written in code, or pseudocode if it supports multiple languages
   2. if applicable, all message types, including errors
       1. a pseudocode block that defines the message object/enum/etc
       2. a table that explains the meanings and suggested action of each message/error
   3. Data models & transformations
      1. Data objects used by exposed APIs (also written in code, or pseudocode for multi-language support, without any explanatory comment)
      2. Field definition table explaining the meaning of each field, with reference to external specifications when applicable
      3. Data transformations (referencing exact data object name, which may include specification by external standards)

### Diagrams: PlantUML (`<project>/diagram/`)

Architecture diagrams, sequence diagrams and activity diagrams are drawn with PlantUML — never hand-drawn in ASCII or HTML tables. Sources (`*.puml`) and exported images (`*.svg`) live in the owning project's own `diagram/` folder — the folder follows its project, never a repo-wide root; the exported SVG is embedded into that project's HTML via `<img>` with a relative path (`diagram/…`). The `.puml` source is authoritative: edit the source, re-export, and commit both together (`plantuml -tsvg <project>/diagram/*.puml`).

1. Choose the form by complexity: general architecture is a UML component diagram; flows with at most one branch decision (fewer than 2 nesting levels) are sequence diagrams; anything with more branching is a UML activity diagram with one swimlane per participant.
2. Labels are written in the documentation language; keep messages short and push details into notes.
3. Do not edit the exported image by hand; do not let the source and the image diverge.
4. Layout grows vertically, not horizontally: branch-heavy flows are written as guard-clause cascades — each branch ends with `stop`, checks run sequentially — never as nested `if/else`, which PlantUML spreads side-by-side.
5. Embedding: each image is wrapped in `<div class="diagram">` and renders at its natural size; the container scrolls horizontally when the image is wider than the page (same mode as `<pre>`). Never scale images down to fit.

## Code
RULES:
- **This document is purely for internal mechanism details. If it is anything relevant to the usage of this project, it should be in the architecture doc instead.**
- **Does not have to be exhaustive. Focus on key execution paths during startup, key events, and shutdown.**
- **Symbols already defined in the architecture document are referenced in CODE.html in the unified one-line form `pub <type> <name>; //见 ARCHITECTURE §xx.xx` — no body, no redefinition. Everything else in CODE.html must be syntactically valid Rust.**
- **No constant definitions for engineering values. Literal values (bus names, periods, capacities, …) are tuned against the real system during development and go stale in docs — they live in the code alone, with the trade-off recorded in the project's implementation notes when it matters. A literal may appear only inline in a call stack, where that flow depends on it. Exception: UI visual literals (component dimensions, radii, font sizes, …) pin down the visual contract and may be stated in the interface/style sections (e.g. the component style spec in ARCHITECTURE); the CSS/code remains the operational source of truth.**
- **Signatures carry no internals. An entry in the internal types & signatures section is a signature plus at most a one-line purpose note. The internal structure of a method (call chains, branching, fallbacks, ordering) is not the signature's responsibility — if it is worth documenting, it belongs to the call-stack section.**
- **Call-stack lines carry calls or judgments only. Every tree line must point at a concrete method call (`module::fn(args)`, DBus method, event send, …); the only exception is a judgment / branch condition. A call may carry at most one trailing comment — a comment is not a tree node: no branch glyph (`├──`/`└──`) of its own, but the ancestor spine (`│`) continues through its line so the tree never breaks. Motivations, policies and trade-offs that do not fit in one comment line live in the prose after the block.**
- **Where no implementation reference exists, draft the code reference — meaningful method names and signatures; the drafted signature carries the comment `待实现，可以根据实际情况调整` (call-stack lines simply use the drafted names, with no marker at the call site). Where a reference exists (main branch, breadpan, std / GTK APIs), use its real names directly.**

-----

1. File tree: the general structure of the final code, explains which module serves what purposes — entries link to their corresponding subsections of the internal types & method signatures chapter (files without a subsection remain plain).
2. CONVENTIONS: shared conventions across interfaces — dedicated chapter right after the first chapter (see "Other constraints"); may stay empty.
3. Internal types & method signatures: further define what each method takes & produces — organized as one subsection per source file, in the file tree's order; each subsection is titled by its file name and links back to the file tree.
4. Call stack tree: what an actual execution of key program logic looks like