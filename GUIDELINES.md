# Documentation Guidelines
Each project should contain 3 documents in HTML:
1. README.html
3. ARCHITECTURE.html
4. CODE.html

And each serves a distinct purpose.

-----

Other constraints:
- Optinally, `reference/` directory can store any official documentation downloaded from internet as a authoritative source of truth.
- Adhere to the single source of truth rule. Any definition should only appear once across the entire project in the most appropriate position, and other appearances should always come in the form of hyperlinks.
- Do not keep any record of amendments to previous versions because it only add confusion to the context.

## Readme
1. WHAT & WHY
	1. IDEAL: the problem we aim to solve
	2. GOAL: what success looks like
2. define user interactions
3. (if applicable) prototype/mock, can be code, can be pictures/videos
## Architecture
RULES:
- **mermaid graph is to be completely avoided. use the borderless-table form for sequence diagrams (see "Sequence diagrams" below), ASCII graph or actual pictures for everything else.**
- **Architecture document should be customer-facing. Everything a customer (app that depends on it, user of this application, etc.) would directly interact with should be documented here. The remaining internal structure and mechanisms should be in CODE.html instead.**
- Optionally, any finer requirement that cannot be described by straightforward mechanisms should be attached to the end of that section as complement. This part should always be the last resort, prefer mechanism over specific constraint whenever possible.

-----

1. TERMS: a table of doamin-specific keywords that should never be synonymized, with their full definition. When these terms are used, no further explaination of the term is allowed.
2. DIAGRAMS: document all relevant architecture as mechanisms, which is the preferred method.
   1. General architecture (how the application interact with others)
   2. Sequence diagrams (how the business logic flows)
3. INTERFACES:
   1. all exposed API specifications written in code, or pseudocode if it supports multiple languages
   2. if applicable, all message types, including errors
       1. a pseudocode block that defines the message object/enum/etc
       2. a table that explains the meanings and suggested action of each message/error
   2. Data models & transformations
      1. Data objects used by exposed APIs (also written in code, or pseudocode for multi-language support, without any explanatory comment)
      2. Field definition table explaining the meaning of each field, with reference to external specifications when applicable
      3. Data transformations (referencing exact data object name, which may include specification by external standards)

### Sequence diagrams: borderless table (`table.seq`)

Sequence diagrams are never drawn in ASCII. Render each of them as a borderless table (`<table class="seq">`; styles live in the root `style.css`) so the vertical lifelines stay aligned regardless of script or font width. Rules:

1. Columns are the regions between adjacent lifelines. Border classes draw the lifelines: first column `lf` (border-left + border-right), middle columns `mid` (border-right), last column `lbl` (border-left).
2. First row is the participant header. Participant names are left-aligned so each label sits on its own lifeline; the row is bold and has no separator line.
3. A cell is empty unless something happens in that region at that step. A light dashed separator is drawn under cells with content only (`td:not(:empty)`), never under empty cells.
4. Alignment semantics — flows hug their origin:
   - rightward message `── text ──→`: left-aligned (`class="l"`)
   - leftward message `←── text`: right-aligned (`class="r"`)
   - everything else (conditions, internal actions, annotations): centered (default)
5. A message must fit on one line. Shorten what is shown in the diagram (e.g. method name without parameters) when needed; full signatures belong in auxiliary tables below the diagram.
6. Cross-region messages (origin and destination are not adjacent lifelines): place the message in the destination's region, right-aligned so the arrowhead lands on the destination lifeline, and note the originator in the destination column (e.g. "（initiated by X）").
7. Annotations spanning several parties may use `colspan` across the affected regions. This interrupts the lifelines for that row and is acceptable for notes, never for messages.

## Code
RULES:
- **This document is purely for internal mechanism details. If it is anything relevant to the usage of this project, it should be in the architecture doc instead.**
- **Does not have to be exhaustive. Focus on key execution paths during startup, key events, and shutdown.**

-----

1. File tree: the general structure of the final code, explains which module serves what purposes
2. Call stack tree: what an actual execution of key program logic looks like
3. Internal types & method signatures: further define what each method takes & produces