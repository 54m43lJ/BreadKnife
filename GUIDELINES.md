# Documentation Guidelines
Each project should contain 4 documents in HTML:
1. README.html
2. BUILD.html
3. ARCHITECTURE.html
4. CODE.html

And each serves a distinct purpose.
-----

Other constraints:
- Optinally, `reference/` directory can store any official documentation downloaded from internet as a authoritative source of truth.
- Adhere to the single source of truth rule. Any definition should only appear once across the entire project in the most appropriate position, and other appearances should always come in the form of links.

## Readme
1. WHAT & WHY
	1. IDEAL: the problem we aim to solve
	2. GOAL: what success looks like
2. define user interactions
3. (if applicable) prototype/mock, can be code, can be pictures/videos
## Build
- Build from source instructions
- including dependencies
## Architecture
RULES:
- **mermaid graph is to be completely avoided. use ASCII graph or actual pictures.**
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
## Code
RULES:
- **This document is purely for internal mechanism details. If it is anything relevant to the usage of this project, it should be in the architecture doc instead.**
- **Does not have to be exhaustive. Focus on key execution paths during startup, key events, and shutdown.**
-----

1. File tree: the general structure of the final code, explains which module serves what purposes
2. Call stack tree: what an actual execution of key program logic looks like
3. Internal types & method signatures: further define what each method takes & produces