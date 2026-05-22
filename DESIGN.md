# Design Document — Invoice & Payment Service

## 1. Data Model

```mermaid
erDiagram
    BUSINESSES ||--o{ API_KEYS : has
    BUSINESSES ||--o{ CUSTOMERS : has
    BUSINESSES ||--o{ INVOICES : owns
    BUSINESSES ||--o{ PAYMENT_ATTEMPTS : owns
    BUSINESSES ||--o{ REFUNDS : owns
    BUSINESSES ||--o{ WEBHOOK_ENDPOINTS : owns
    CUSTOMERS ||--o{ INVOICES : receives
    INVOICES ||--o{ INVOICE_LINE_ITEMS : contains
    INVOICES ||--o{ PAYMENT_ATTEMPTS : has
    PAYMENT_ATTEMPTS ||--o{ REFUNDS : refunded_by
    WEBHOOK_ENDPOINTS ||--o{ WEBHOOK_DELIVERIES : receives

    BUSINESSES {
        uuid id PK
        text name
        timestamptz created_at
    }

    API_KEYS {
        uuid id PK
        uuid business_id FK
        text key_prefix
        text key_hash
        timestamptz revoked_at
    }

    CUSTOMERS {
        uuid id PK
        uuid business_id FK
        text name
        text email
    }

    INVOICES {
        uuid id PK
        uuid business_id FK
        uuid customer_id FK
        bigint total_cents
        invoice_state state
        date due_date
    }

    INVOICE_LINE_ITEMS {
        uuid id PK
        uuid invoice_id FK
        text description
        bigint quantity
        bigint unit_amount_cents
    }

    PAYMENT_ATTEMPTS {
        uuid id PK
        uuid business_id FK
        uuid invoice_id FK
        text idempotency_key
        payment_attempt_status status
        text psp_ref
    }

    REFUNDS {
        uuid id PK
        uuid business_id FK
        uuid payment_attempt_id FK
        text idempotency_key
        bigint amount_cents
        refund_state state
        text psp_ref
    }

    WEBHOOK_ENDPOINTS {
        uuid id PK
        uuid business_id FK
        text url
        text signing_secret
    }

    WEBHOOK_DELIVERIES {
        uuid id PK
        uuid business_id FK
        uuid endpoint_id FK
        text event_type
        webhook_delivery_status status
        int attempt_count
    }
```

| Table | Primary key | Important indexes |
|-------|-------------|-------------------|
| `businesses` | UUID | — |
| `api_keys` | UUID | `(key_prefix)` partial where not revoked |
| `customers` | UUID | `(business_id)` |
| `invoices` | UUID | `(business_id, state)`, `(customer_id)` |
| `invoice_line_items` | UUID | `(invoice_id)` |
| `payment_attempts` | UUID | **UNIQUE `(business_id, idempotency_key)`**, `(invoice_id)` |
| `refunds` | UUID | **UNIQUE `(business_id, idempotency_key)`**, `(payment_attempt_id, state)` |
| `webhook_endpoints` | UUID | `(business_id)` |
| `webhook_deliveries` | UUID | `(status, next_retry_at)` where pending |

**Why this shape:** Invoices and line items are normalized so totals are computed server-side from immutable line rows. Payment attempts are first-class rows rather than JSON blobs so idempotency keys, PSP references, and retry state are queryable. Webhook deliveries are decoupled from API handlers in their own outbox table.

**At 100× scale:** Partition `webhook_deliveries`, `payment_attempts`, and `refunds` by time or business_id, add read replicas for list endpoints, and move webhook/refund delivery to a dedicated queue (SQS/Kafka) with consumer workers.

## 2. Invoice State Machine

```mermaid
stateDiagram-v2
    state "open" as Open
    state "paid" as Paid
    state "void" as Void
    state "uncollectible" as Uncollectible

    [*] --> Open: "POST /invoices"
    Open --> Paid: "payment succeeded"
    Open --> Void: "POST /invoices/{id}/void"
    Open --> Uncollectible: "reserved for future dunning"
    Paid --> [*]
    Void --> [*]
    Uncollectible --> [*]
```

| Transition | Trigger | Terminal? |
|------------|---------|-----------|
| → `open` | Invoice created | No |
| `open` → `paid` | PSP success | Yes |
| `open` → `void` | Explicit void API | Yes |
| `open` → `uncollectible` | Not implemented in MVP | Yes |

**Reversible transitions:** None. Terminal states are immutable.

**Invalid transitions:** Rejected with `409 Conflict` and error code `invalid_state_transition`. Examples: paying a `paid` invoice, voiding a `paid` invoice.

The `draft` enum value exists for future use but invoices are created directly in `open` per product choice (immediately payable).

## 3. Payment Correctness & Failure Modes

**Concurrency mechanism:** PostgreSQL row-level lock (`SELECT … FOR UPDATE` on the invoice row) plus an in-flight guard (`processing`/`pending` attempts) plus idempotency keys.

### (a) Two concurrent POST /pay on the same invoice

Both requests serialize on the invoice row lock. The first inserts a `processing` attempt and proceeds. The second sees an in-flight attempt (or a terminal invoice state after the first completes) and returns `409 Conflict`. Outcome: at most one successful charge.

### (b) PSP timeout (`tok_timeout`, 30s)

The API-facing PSP client uses a **5-second timeout**. On timeout the attempt is marked `pending`, invoice stays `open`, and the handler returns **`202 Accepted`**. A background reconciler retries with a **35-second timeout**, completes the charge, and updates invoice state + webhooks. Callers poll `GET /invoices/{id}`.

### (c) PSP success but crash before persist

The idempotency key is stored before calling the PSP. If the handler crashes after PSP success but before persisting the response, the client retries with the same key. The stored attempt row is found and the cached response is replayed — no second PSP call. In production we would also pass idempotency keys to the PSP.

### (d) Idempotency key reused with different body

We hash the request body at payment time. If the same `(business_id, idempotency_key)` exists with a different hash → **`422 Unprocessable Entity`** (`idempotency_key_reused`).

### (e) POST /pay on a paid invoice

Inside the locked transaction the invoice state is checked. `paid` is not payable → **`409 Conflict`**.

**Why row lock over alternatives:** Advisory locks would work but are easier to leak on connection pool churn. Serializable isolation is heavier than needed. Optimistic concurrency alone does not serialize two simultaneous pay attempts cleanly. `FOR UPDATE` is explicit, well-understood, and scoped to one invoice.

## 4. Webhook Design

**Signing:** HMAC-SHA256 over `{unix_timestamp}.{raw_json_body}` using the endpoint signing secret. Headers: `X-Webhook-Signature`, `X-Webhook-Timestamp`, `X-Webhook-Id`.

**Replay protection:** Receivers should reject timestamps older than 5 minutes.

**Retry policy:** 6 attempts at **0s, 30s, 2m, 10m, 1h, 6h** (~7.5 hours total). Failed deliveries after max attempts are marked `dead` and logged.

**Dead queue reconciliation:** `webhook_deliveries.status = dead` is the local dead-letter queue. A reconciliation job should periodically scan dead deliveries, group them by business and event type, and compare them against the source tables (`invoices`, `payment_attempts`, `refunds`) to decide whether to replay, suppress, or escalate. Replays should create a new delivery row linked to the original dead delivery rather than mutating history; suppressions should record an operator reason; repeated dead deliveries should page support or surface in an admin dashboard.

**Missed events:** Businesses reconcile via `GET /invoices` and periodic full sync; production would add a signed event log API.

**Decoupling:** API handlers insert into `webhook_deliveries` and return. A background worker polls due rows and POSTs to registered URLs — webhook latency never blocks payment responses.

## 5. Refund Pipeline Design

Refunds are not implemented in the MVP, but the production design should treat refunds as their own ledgered workflow rather than directly mutating payment attempts.

```mermaid
stateDiagram-v2
    state "requested" as Requested
    state "processing" as Processing
    state "succeeded" as Succeeded
    state "failed" as Failed
    state "requires_reconciliation" as RequiresReconciliation
    state "canceled" as Canceled

    [*] --> Requested: "POST /refunds"
    Requested --> Processing: "worker claims refund"
    Processing --> Succeeded: "PSP refund succeeded"
    Processing --> Failed: "PSP refund rejected"
    Processing --> RequiresReconciliation: "timeout or ambiguous PSP result"
    RequiresReconciliation --> Processing: "safe retry"
    RequiresReconciliation --> Succeeded: "PSP confirms refunded"
    RequiresReconciliation --> Failed: "PSP confirms not refunded"
    Requested --> Canceled: "cancel before PSP call"
    Succeeded --> [*]
    Failed --> [*]
    Canceled --> [*]
```

Suggested tables:

| Table | Purpose |
|-------|---------|
| `refunds` | One row per refund request, including amount, idempotency key, state, PSP reference, and failure code |
| `refund_events` | Append-only audit trail of state transitions and PSP responses |
| `refund_dead_queue` | Ambiguous or exhausted refund jobs requiring reconciliation/operator review |

Refund rules:

1. A refund must reference a `succeeded` payment attempt.
2. `amount_cents` must be positive and cannot exceed the unrefunded amount for that payment attempt.
3. Partial refunds are allowed by inserting multiple refund rows until the refundable balance reaches zero.
4. Idempotency is enforced with `UNIQUE (business_id, idempotency_key)`.
5. The PSP refund idempotency key should be derived from the internal refund id so worker retries do not double-refund.

Pipeline:

1. `POST /v1/refunds` validates the payment attempt, computes remaining refundable balance under a row lock, inserts `refunds.state = requested`, and returns quickly.
2. A refund worker claims requested rows using `FOR UPDATE SKIP LOCKED`, marks them `processing`, and calls the PSP refund API.
3. PSP success marks the refund `succeeded`, appends a `refund_events` row, and enqueues `refund.succeeded`.
4. PSP business failure marks the refund `failed`, stores `failure_code`, appends an event, and enqueues `refund.failed`.
5. PSP timeout/network ambiguity marks the refund `requires_reconciliation`; the refund reconciler polls PSP by refund id/reference before retrying.
6. If reconciliation exceeds retry limits, the refund is copied to `refund_dead_queue` with the last PSP response and operator instructions. Operators should never manually mark a refund succeeded without PSP evidence.

Invoice relationship:

- A fully refunded invoice should not move back from `paid` to `open`; it remains `paid` with derived refund status (`partially_refunded` or `refunded`) calculated from refund totals.
- This avoids mixing collection state with money-movement reversal state.

Webhook events:

- `refund.created`
- `refund.succeeded`
- `refund.failed`
- `refund.requires_reconciliation`

## 6. API Key Model

- **Generation:** `dodo_live_` + 32 random alphanumeric characters.
- **Storage:** SHA-256 hash + 8-character prefix for lookup. Plaintext never stored.
- **Transmission:** `Authorization: Bearer <key>` over HTTPS (TLS assumed in production).
- **Rotation:** Issue new key via admin endpoint (not built); revoke old key by setting `revoked_at`.
- **Revocation:** Soft-delete via `revoked_at`; middleware rejects revoked keys.
- **Blast radius:** A leaked key grants full access to that business's data. Prefix display helps identify which key leaked without exposing the secret.

## 7. What We Cut and Why

1. **Refund API implementation** — designed above but out of MVP scope; it needs PSP refund APIs, refund ledger tables, and operator reconciliation tooling.
2. **Subscriptions / recurring billing** — different product surface entirely.
3. **Multi-currency / tax** — assignment explicitly excluded; keeps money path integer USD cents only.
4. **Rate limiting** — discussed in production gaps; not needed for correctness demo.
5. **Email notifications** — webhooks cover business notification; email is redundant for MVP.

## 8. Production Readiness Gaps

1. **Observability** — no metrics, structured trace IDs, or alerting on dead webhook/refund queue growth.
2. **Rate limiting & abuse protection** — API keys authenticate but do not throttle brute-force or pay spam.
3. **Audit log** — no immutable record of who changed invoice state or revoked keys.
