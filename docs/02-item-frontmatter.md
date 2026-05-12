# docs/02-item-frontmatter.md

> YAML frontmatter schema for every `.md` item file. Source of truth for what fields each kind carries.

## Format

```
---
<YAML frontmatter>
---

<Markdown body>
```

The frontmatter is YAML 1.2. The body is CommonMark Markdown. The `---` delimiters are required even on items with empty bodies.

## Base frontmatter (every kind)

```yaml
id: 01J5K3R0H9NMVE5T8ZQA3W7BJP        # ULID — sortable by creation time
schema_version: 1                      # bump only on breaking changes
kind: email                            # one of the kinds listed below
matter_id: 01J5K1Z6...                 # owning matter; "_unfiled" for unfiled
client_id: 01J5K1Z5...                 # denormalized for greppability
created_at: 2026-05-08T14:32:11Z       # RFC 3339, UTC
updated_at: 2026-05-09T09:15:00Z
content_hash: sha256:e3b0c44298...     # of the body below
title: "Re: Stevens deposition"        # display title
tags: [discovery, scheduling]          # user-editable strings
isolation_mode: shared                 # inherited from matter; cached here
attachments: []                        # see below
source: {}                             # per-kind provenance
custom: {}                             # user escape hatch — Mike never reads
```

### `attachments` shape

```yaml
attachments:
  - sha256: 84a2b97f...c1              # full sha256
    filename: "redlined-contract-v3.pdf"  # original filename
    mime: application/pdf
    size_bytes: 8423091
```

Each attachment refers to a file in `<matter>/attachments/<sha256-prefix>.<ext>`. The backend manages ref-counting via the `attachments` SQLite table; the `.md` is the only thing that names what an attachment is for.

## Kinds

### `kind: document`

Body: Docling's Markdown output (preserves headings, tables, lists). Page boundaries marked with invisible HTML comments:

```markdown
<!-- page: 1 -->
# Memorandum of Understanding

...

<!-- page: 2 -->
2. The parties agree...
```

The chunker reads these comments to populate `chunks.page`.

Additional frontmatter:
```yaml
source:
  kind: upload                         # upload | docling | external_edit
  original_filename: "MOU-v3.pdf"
  uploaded_at: 2026-05-08T14:32:00Z
  page_count: 12
  detected_tables: 3
  parser: docling@1.0.0                # or "pdfium" for the fast path
```

### `kind: email`

Body: email body in Markdown. HTML emails are normalized via `html2md`. Quoted history preserved with `>` blockquotes. Signatures separated by `-- ` (RFC 3676 sig marker).

```markdown
Hi Sarah,

Confirming Tuesday at 10am for the Stevens deposition...

Best,
Dave

-- 
Dave Woodes
Partner, Smith & Smith LLP
```

Additional frontmatter:
```yaml
source:
  kind: gmail                          # gmail | imap | graph | manual
  account: you@yourfirm.example
  message_id: <CAB+xyz@mail.gmail.com>
  thread_id: 18f2d3a...
  imap_uid: 18472                      # gmail or imap only
  received_at: 2026-05-08T14:32:11Z
  internal_date: 2026-05-08T14:31:58Z

participants:
  from:
    name: "John Smith"
    email: counsel@otherfirm.example
  to:
    - { name: "You", email: you@yourfirm.example }
  cc:
    - { name: "Partner", email: partner@yourfirm.example }
  bcc: []

links:
  replies_to: 01J5K3Q1...              # parent email's item id, if known
```

### `kind: note`

Body: pure user-written Markdown. No structure imposed.

Additional frontmatter:
```yaml
source:
  kind: manual                         # always 'manual' for v1
  author: local-user
```

### `kind: appointment`

Body: the appointment description (freeform), if any.

Additional frontmatter:
```yaml
appointment:
  start: 2026-05-12T10:00:00Z
  end: 2026-05-12T11:00:00Z
  timezone: America/Los_Angeles
  location: "Conference Room B / Zoom: https://..."
  organizer:
    name: "Dave Woodes"
    email: you@yourfirm.example
  attendees:
    - { name: "Opposing Counsel", email: counsel@otherfirm.example, response: accepted }
  recurrence_rule: null                # RFC 5545 RRULE or null

source:
  kind: caldav                         # caldav | google_cal | graph | manual
  account: you@yourfirm.example
  uid: "1f3a-9c2b-...-acme.ics"
  href: "/dav/calendars/.../1f3a.ics"
  etag: '"6f0a..."'
```

### `kind: contact`

Body: freeform notes about the contact.

Additional frontmatter:
```yaml
contact:
  full_name: "Sarah O'Neill"
  emails:
    - { address: sarah@otherfirm.example, primary: true }
  phones:
    - { number: "+1-415-555-0123", kind: mobile }
  organization: "Other Firm LLP"
  title: "Senior Counsel"
  addresses: []

source:
  kind: manual                         # manual | gmail_contacts | graph_contacts
```

### `kind: chat`

Body: the chat transcript. Format:

````markdown
## User
Summarize the most recent email from Stevens counsel.

## Assistant
<!-- model: claude-3-7-sonnet, tokens: 412 -->
Stevens counsel confirmed the deposition for Tuesday at 10am [1].

```citation
ref: 1
item_id: 01J5K3R0H9NMVE5T8ZQA3W7BJP
chunk_id: 01J5K3R0...001
quote: "Confirming Tuesday at 10am for the Stevens deposition"
```

## User
Schedule a prep meeting.

...
````

Additional frontmatter:
```yaml
chat:
  model_default: claude-3-7-sonnet
  message_count: 8

source:
  kind: manual
```

## Validation

Every `.md` write is validated against the JSON Schema generated from Rust types. Invalid frontmatter fails the write with a clear error. The schema is published at `packages/shared/openapi.json` and as standalone JSON Schemas in `packages/shared/src/types/`.

## Reading and writing

Single owner: `backend/src/storage/`. Per anti-pattern #5, nothing else writes `.md` files.

The frontend never parses `.md` directly — it gets normalized JSON via the backend API. External editors (VS Code, Obsidian) are read/write but Mike treats their writes as untrusted input and re-validates on watcher fire.

## Schema evolution

Per `docs/04-versioning.md`:
- New optional fields: no version bump required; old items silently miss them.
- New required fields: requires a migration from v1 → v2 that adds defaults.
- Renamed or restructured fields: requires explicit migration. Old items rewritten on read, atomically.

Migrations live at `backend/src/storage/migrations/frontmatter_v{n}_to_v{n+1}.rs`, tested with golden files in `tests/fixtures/frontmatter/`.
