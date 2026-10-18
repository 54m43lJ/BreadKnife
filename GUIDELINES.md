# Documentation Guidelines
Each project should contain the following documents in HTML:
1. README.html
2. USER.html — the contract toward end users / dependent software: their protocols, conventions, APIs and workflows.
3. CORE.html — internal mechanics only: internally-accessed data structures & APIs, and the internal run logic (call stacks).
4. PROVIDER.html — the contract toward underlying data sources / dependencies: their protocols, conventions, APIs and workflows; exists only when the project has such a boundary, omitted otherwise.
5. BUILD.html — produced once development is complete: it guides other developers in using and building on
   the project; it is not written during the design phase, but it is a document the project keeps.

The three development documents split by boundary: whatever crosses the upper boundary (end users, dependent software) is USER; whatever never leaves the project is CORE; whatever crosses the lower boundary (data sources, services, hardware the project depends on) is PROVIDER. Anything with no counterpart outside the project belongs to CORE alone.

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
- Section layout only applies to the development documents — USER, CORE and PROVIDER: the opening may carry paragraph-form qualifying text (when structured representation is not possible); the middle must be structured content — code blocks/diagrams/tables — never prose; the end may carry bulleted-point supplements (`ul.note`) only.
- An in-code comment carrying more than three points of information becomes a trailing supplement list (`ul.note`) after its code block; the comment itself keeps only what the signature or the call needs.
- A code block must contain a single continuous content. Never pack disjoint pieces (e.g., two unrelated flows) into one code block — split them into separate code blocks or subsections.
- USER, CORE and PROVIDER each keep a dedicated chapter for conventions shared across interfaces — placed right after the first chapter; it may stay empty. A convention is stated once there and referenced from the affected sections instead of being repeated.

## Readme
1. WHAT & WHY
	1. IDEAL: the problem we aim to solve
	2. GOAL: what success looks like
2. define user interactions
3. (if applicable) prototype/mock, can be code, can be pictures/videos
## User
RULES:
- **This document is customer-facing. Everything a customer (an app that depends on it, a user of this application, etc.) would directly interact with — protocols, conventions, APIs, workflows — is documented here. Internal structure and mechanisms go to CORE.html; everything toward underlying data sources / dependencies goes to PROVIDER.html.**
- Optionally, any finer requirement that cannot be described by straightforward mechanisms should be attached to the end of that section as complement. This part should always be the last resort, prefer mechanism over specific constraint whenever possible.

-----

1. TERMS: a table of doamin-specific keywords that should never be synonymized, with their full definition. When these terms are used, no further explaination of the term is allowed.
2. CONVENTIONS: shared conventions across interfaces — dedicated chapter right after the first chapter (see "Other constraints"); may stay empty.
3. DIAGRAMS: document all relevant architecture as mechanisms, which is the preferred method.
   1. General architecture (how the application interact with others)
   2. Workflows: how the program logic flows, in business language only, minimal amount of terms and API usage is allowed when referring to well-known standards — sequence diagrams / swimlane activity diagrams, form chosen per "Diagrams: PlantUML"
4. INTERFACES:
   1. all exposed API specifications written in code, or pseudocode if it supports multiple languages
   2. if applicable, all message types, including errors
       1. a pseudocode block that defines the message object/enum/etc
       2. a table that explains the meanings and suggested action of each message/error
   3. Data models & transformations
      1. Data objects used by exposed APIs (also written in code, or pseudocode for multi-language support, without any explanatory comment)
      2. Field definition table explaining the meaning of each field, with reference to external specifications when applicable
      3. Data transformations (referencing exact data object name, which may include specification by external standards)

## Core
RULES:
- **This document is purely for internal mechanism details: data structures and APIs that only the project itself accesses, and the internal run logic as call stack trees. If it is anything relevant to the usage of this project, it belongs to USER.html; if it is about reaching an underlying data source / dependency, it belongs to PROVIDER.html.**
- **Does not have to be exhaustive. Focus on key execution paths during startup, key events, and shutdown.**
- **Symbols already defined in USER.html or PROVIDER.html are referenced in CORE.html in a unified one-line declaration form with the trailing marker `//见 USER §xx.xx` or `//见 PROVIDER §xx.xx`, written in the project development language — no body, no redefinition. Everything else in CORE.html must be syntactically valid in the project development language.**
- **No constant definitions for engineering values. Literal values (bus names, periods, capacities, …) are tuned against the real system during development and go stale in docs — they live in the code alone, with the trade-off recorded in the project's implementation notes when it matters. A literal may appear only inline in a call stack, where that flow depends on it. Exception: UI visual literals (component dimensions, radii, font sizes, …) pin down the visual contract and may be stated in the interface/style sections (e.g. the component style spec in USER); the CSS/code remains the operational source of truth.**
- **Signatures carry no internals. An entry in the internal types & signatures section is a signature plus at most a one-line purpose note. The internal structure of a method (call chains, branching, fallbacks, ordering) is not the signature's responsibility — if it is worth documenting, it belongs to the call-stack section.**
- **Call-stack lines carry calls or judgments only. Every tree line must point at a concrete method call (`module::fn(args)`, DBus method, event send, …); the only exception is a judgment / branch condition. A call may carry at most one trailing comment — a comment is not a tree node: no branch glyph (`├──`/`└──`) of its own, but the ancestor spine (`│`) continues through its line so the tree never breaks. Motivations, policies and trade-offs that do not fit in one comment line live in the prose after the block.**
- **Where no implementation reference exists, draft the code reference — meaningful method names and signatures; the drafted signature carries the comment `待实现，可以根据实际情况调整` (call-stack lines simply use the drafted names, with no marker at the call site). Where a reference exists (main branch, breadpan, std / GTK APIs), use its real names directly.**

-----

1. File tree: the general structure of the final code, explains which module serves what purposes — entries link to their corresponding subsections of the internal types & method signatures chapter (files without a subsection remain plain).
2. CONVENTIONS: shared conventions across interfaces — dedicated chapter right after the first chapter (see "Other constraints"); may stay empty.
3. Internal types & method signatures: further define what each method takes & produces — organized as one subsection per source file, in the file tree's order; each subsection is titled by its file name and links back to the file tree.
4. Call stack tree: what an actual execution of key program logic looks like

## Provider
This document exists only when the project depends on underlying data sources or services (system services, IPC buses, hardware, daemons, network endpoints, …); with nothing below it, the project omits it.

RULES:
- **This document is provider-facing. Everything toward the underlying data sources / dependencies — their protocols, conventions, APIs and the workflows that drive them — is documented here. Where these providers sit in the whole system is the general architecture diagram in USER.html; how their data is handled inside the project is CORE.html.**
- **Only the consumed subset is contractual here. The full, authoritative definition of a foreign protocol is its official documentation (mirrored under `reference/` when applicable) and is linked, never copied.**
- **Workflow diagrams on this boundary name the provider's methods/messages as they cross it; the project's internal call structure does not appear — the call stack tree in CORE.html owns that.**

-----

1. TERMS: a table of provider-specific keywords that should never be synonymized, with their full definition. When these terms are used, no further explaination of the term is allowed.
2. CONVENTIONS: shared conventions across the provider boundary — dedicated chapter right after the first chapter (see "Other constraints"); may stay empty.
3. DIAGRAMS: workflows on the provider boundary — sequence diagrams / swimlane activity diagrams, form chosen per "Diagrams: PlantUML".
4. INTERFACES:
   1. all consumed API specifications written in code, or pseudocode if it supports multiple languages
   2. if applicable, all message types the project sends or receives on this boundary, including errors
      1. a pseudocode block that defines the message object/enum/etc
      2. a table that explains the meanings and suggested action of each message/error
   3. Data models & transformations
      1. Data objects as they cross this boundary (also written in code, or pseudocode for multi-language support, without any explanatory comment)
      2. Field definition table explaining the meaning of each field, with reference to the external specification (the authoritative source) when applicable
      3. Data transformations between provider data objects and the project's own data objects (referencing exact data object names)

## Diagrams: PlantUML (`<project>/diagram/`)

Mermaid graphs are to be completely avoided; ASCII graphs or actual pictures for everything else. Architecture diagrams, sequence diagrams and activity diagrams in USER.html and PROVIDER.html are drawn with PlantUML — never hand-drawn in ASCII or HTML tables. Sources (`*.puml`) and exported images (`*.svg`) live in the owning project's own `diagram/` folder — the folder follows its project, never a repo-wide root; the exported SVG is embedded into that project's HTML via `<img>` with a relative path (`diagram/…`). The `.puml` source is authoritative: edit the source, re-export, and commit both together (`plantuml -tsvg <project>/diagram/*.puml`).

1. Choose the form by complexity: general architecture is a UML component diagram; flows with at most one branch decision (fewer than 2 nesting levels) are sequence diagrams; anything with more branching is a UML activity diagram with one swimlane per participant.
2. Labels are written in the documentation language; keep messages short and push details into notes.
3. Do not edit the exported image by hand; do not let the source and the image diverge.
4. Layout grows vertically, not horizontally: branch-heavy flows are written as guard-clause cascades — each branch ends with `stop`, checks run sequentially — never as nested `if/else`, which PlantUML spreads side-by-side.
5. Embedding: each image is wrapped in `<div class="diagram">` and renders at its natural size; the container scrolls horizontally when the image is wider than the page (same mode as `<pre>`). Never scale images down to fit.
