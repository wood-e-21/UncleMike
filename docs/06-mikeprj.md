# docs/06-mikeprj.md

> Portable encrypted matter bundle format. Inherits design from MikeRust with our context applied.

## Use case

A lawyer wants to hand a matter to opposing counsel, archive to a client, or send to themselves for offline review. The recipient gets a single file, plays it through Mike, sees the matter exactly as the sender saw it.

## File layout (v1)

```
┌────────────────────────────────────────┐
│ magic   : "MIKEPRJ\0"   8 bytes        │
│ version : u8            1 byte         │
│ flags   : u8            1 byte         │
│ email_h : [u8; 32]     SHA-256(email)  │
│ salt    : [u8; 16]     Argon2id salt   │
│ nonce   : [u8; 12]     AES-GCM nonce   │
│ payload : variable     ciphertext      │
└────────────────────────────────────────┘
```

Header is 70 bytes. Payload is AES-256-GCM-encrypted ZIP archive.

## ZIP payload contents

```
manifest.json                  schema_version, exporter, created_at
client.md                      copy of <matter>/client.md (or just the client metadata)
matter.md                      copy of <matter>/matter.md
items/                         all .md item files from the matter
  document-<ulid>.md
  email-<ulid>.md
  ...
attachments/                   all binary attachments referenced by items
  <sha256-prefix>.pdf
  ...
chats/                         optional, only when --include-chats
  chat-<ulid>.md
audit-excerpt.json             optional, only when --include-audit
```

The structure mirrors `<workspace>/matters/<client>/<matter>/` exactly. Importing is a directory copy after decryption.

## Manifest

```json
{
  "schema_version": 1,
  "mike_version": "0.3.0",
  "exporter": {
    "workspace_id": "01J...",
    "user_label": "Dave Woodes",
    "exported_at": "2026-05-12T14:00:00Z"
  },
  "matter": {
    "id": "01J5K1Z6...",
    "name": "Stevens Litigation",
    "client_name": "Acme Corp",
    "item_count": 47,
    "attachment_count": 8,
    "size_bytes_uncompressed": 38291441
  },
  "options": {
    "include_chats": false,
    "include_audit": false
  }
}
```

## Cryptography

**Key derivation:** Argon2id with parameters `(memory = 64 MiB, t = 3, p = 1)`. Salt is per-file (random 16 bytes in header). Input is the normalized recipient email (lowercase, trimmed).

**Cipher:** AES-256-GCM. Key derived above. Nonce in header. Associated Authenticated Data (AAD = data that's authenticated but not encrypted, so any tampering is detected) is the entire fixed-size header before the nonce — meaning a flipped bit in the email hash or salt fails decryption.

**Authentication:** GCM's built-in MAC. Tampering anywhere in the ciphertext or AAD fails decryption.

**Email normalization:**
```
trim() → to_ascii_lowercase() → unicode_normalization::nfc()
```

Then SHA-256 the result. Both the hash (in header) and the derived key (for AES) use this normalized form.

## Threat model and explicit limits

### What v1 protects against

- Casual interception in transit (the file is opaque to anyone without the email).
- Accidental leak of an exported file to the wrong person on the same firm — wrong email = no key.
- A misplaced USB stick containing the file — unreadable without the email.

### What v1 does NOT protect against

- An attacker who knows the recipient's email and possesses the file. They can decrypt it. Period.
- Brute-force of common emails. Argon2id slows this down but doesn't make it infeasible for a single known target.
- The recipient leaking the decrypted contents after import.

**This is "weak email pinning."** Documented at the top of every export UI. Suitable for inter-counsel exchange where the threat is "wrong inbox," not "active attacker with target acquisition."

## v2 design (deferred)

v2 adds **authenticated sharing** via out-of-band PIN:
1. Exporter generates a one-time 6-digit PIN.
2. Exporter delivers the PIN through a different channel (Signal, phone call).
3. File is encrypted with key derived from `(email || PIN)`.
4. Without the PIN, knowing the email is not enough.

v2 file format adds a `flags` bit and a separate PIN salt. Backward compatible (v1 files still readable by v2 binaries).

Slot v2 for ~6 months after alpha, when the user base is large enough to justify the UX cost (managing PINs).

## Export flow

1. User picks a matter and types recipient email.
2. Backend confirms — show "This file will be readable by anyone who has it AND knows `<email>`. Strong sharing requires v2 (not yet shipped)."
3. Backend reads the entire matter folder (items + attachments + chats if requested).
4. ZIP into memory.
5. Derive key via Argon2id.
6. AES-GCM encrypt with random nonce, AAD = header.
7. Write to `<workspace>/matters/<client>/<matter>/exports/<matter-slug>-<date>.mikeprj`.
8. Audit log: `mikeprj.export {matter_id, recipient_email_hash, size_bytes}`.

## Import flow

1. User drags `.mikeprj` into Mike (or `File → Import Matter`).
2. Backend reads header. Verifies magic + version.
3. Computes hash of user's normalized email. Compares to header's `email_hash`.
4. If mismatch: refuse with "This file is not addressed to your account email (`<email>`). Ask the sender to re-export."
5. If match: derive key, decrypt, verify GCM tag.
6. If tag fails: refuse with "File is corrupted or tampered with."
7. Unzip into a temp directory.
8. Read manifest, sanity-check.
9. If recipient already has a matter with same `id`: prompt "Matter exists. Overwrite, merge, or import as new?"
10. Move into `<workspace>/matters/_imported/<client-slug>/<matter-slug>/`. The recipient can then re-home it to their preferred client.
11. Run rebuild on the imported subset to populate SQLite.
12. Audit log: `mikeprj.import {matter_id, sender_workspace_id}`.

## Storage location

Exports live under the matter's own folder at `<matter>/exports/`. This means:
- A matter folder is fully self-contained including its export artifacts.
- A `tar` of the matter folder includes any past exports.
- The lawyer can find "the thing I sent to opposing counsel last month" by browsing the matter.

## Performance targets

For a 5-matter workspace (avg 200 items, 50 attachments, ~500MB total):

- Export single matter (100 items, 8MB): < 5 seconds wall time.
- Import single matter: < 10 seconds (includes SQLite rebuild on subset).

Bigger matters scale roughly linearly; document in `docs/09-performance.md`.

## Anti-features

- **No partial export.** Either the whole matter goes or nothing. Lawyer wants "just the privileged docs"? They create a new matter with only those items and export that.
- **No cross-matter export.** A bundle = one matter. Sharing five matters = five bundles.
- **No re-encryption without re-export.** Can't change the recipient on an existing `.mikeprj`. Make a new one.
