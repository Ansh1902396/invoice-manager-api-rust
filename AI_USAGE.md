# AI Usage Disclosure

## Tools used

- **Cursor (Claude)** — primary implementation assistant for scaffolding the Rust workspace, Axum routes, SQLx migrations, docker-compose setup, integration tests, and documentation drafts.
- **Cursor autocomplete** — boilerplate for serde structs, SQL queries, and test helpers.

## Three decisions made independently of AI defaults

1. **Invoice created in `open` state (not `draft`)**  
   AI suggested keeping a draft → open finalize step. I chose immediate `open` to reduce API surface while still documenting `draft` in the state enum for future use. Fewer endpoints, same correctness for the assignment demo.

2. **Row lock + in-flight attempt guard (not serializable isolation)**  
   AI initially leaned toward serializable transactions globally. I chose `SELECT FOR UPDATE` on the invoice row plus rejecting duplicate `processing`/`pending` attempts because it is narrower, easier to reason about in logs, and avoids serializable retry storms under concurrent pay load.

3. **202 Accepted + background reconciler for PSP timeouts**  
   AI proposed blocking until PSP responds or immediately failing the payment. I chose a short API timeout (5s), `pending` attempt state, and a 35s reconciler so `tok_timeout` never hangs the API while still eventually completing payment — matching how real billing systems treat slow PSPs.

## Something the AI got wrong (and how I verified)

The initial migration seed used a placeholder SHA-256 hash for the demo API key instead of the real digest. I caught this by running `shasum -a 256` on the demo key string and updating the migration before testing auth. Verified by curling `/v1/customers` with the documented demo key after `docker compose up`.

## What I reviewed manually

- All money paths use `i64` cents (no floats).
- Idempotency replay path returns stored JSON without a second PSP call.
- Concurrent pay test asserts exactly one `200 OK` and nine `409 Conflict` responses.
- Webhook worker retry intervals match DESIGN.md.
