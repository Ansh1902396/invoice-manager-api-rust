# Invoice & Payment Service

Minimal invoice and payment API built for the Dodo Payments backend take-home assignment.

## Quick start

```bash
docker compose up --build
```

Services:
- Invoice API: http://localhost:8080
- Mock PSP: http://localhost:8081
- PostgreSQL: localhost:5432

Demo API key (seeded on migration):

```
dodo_live_demokey1234567890123456789012
```

Health check:

```bash
curl http://localhost:8080/health
```

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | `postgres://postgres:postgres@localhost:5432/invoices` | PostgreSQL connection |
| `PSP_URL` | `http://localhost:8081/charge` | Mock PSP charge endpoint |
| `PSP_TIMEOUT_SECS` | `5` | API-facing PSP HTTP timeout |
| `PSP_RECONCILE_TIMEOUT_SECS` | `35` | Background reconciler timeout |
| `LISTEN_ADDR` | `0.0.0.0:8080` | Invoice service bind address |

## curl examples

Set the API key once:

```bash
export API_KEY="dodo_live_demokey1234567890123456789012"
```

### 1. Create a customer

```bash
curl -s -X POST http://localhost:8080/v1/customers \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name":"Acme Corp","email":"billing@acme.example"}' | jq
```

### 2. Create an invoice

```bash
CUSTOMER_ID="<customer-id-from-above>"

curl -s -X POST http://localhost:8080/v1/invoices \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d "{
    \"customer_id\": \"$CUSTOMER_ID\",
    \"due_date\": \"2026-12-31\",
    \"line_items\": [
      {\"description\": \"Pro plan\", \"quantity\": 1, \"unit_amount_cents\": 9900}
    ]
  }" | jq
```

### 3. Pay successfully

```bash
INVOICE_ID="<invoice-id-from-above>"

curl -s -X POST "http://localhost:8080/v1/invoices/$INVOICE_ID/pay" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Idempotency-Key: pay-success-1" \
  -H "Content-Type: application/json" \
  -d '{"card_token":"tok_success"}' | jq
```

### 4. Pay with declined card (on a new open invoice)

Create another invoice, then:

```bash
curl -s -X POST "http://localhost:8080/v1/invoices/$INVOICE_ID/pay" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Idempotency-Key: pay-decline-1" \
  -H "Content-Type: application/json" \
  -d '{"card_token":"tok_card_declined"}' | jq
```

## Tests

Requires Postgres and mock PSP running (`docker compose up`).

```bash
cargo test -p invoice-service --test integration
```

Tests cover:
- Concurrent `POST /pay` — at most one success, invoice ends `paid`
- Idempotency — same key returns same response, one succeeded attempt
- PSP network error — fast `202 Accepted`, invoice stays `open`, attempt `pending`

## Demo Video

<!-- Replace with your Loom / Drive link before submission -->
https://example.com/your-demo-video

## API documentation

See [openapi.yaml](./openapi.yaml).

## Design

See [DESIGN.md](./DESIGN.md).

## AI usage

See [AI_USAGE.md](./AI_USAGE.md).

## What's left before submission

The implementation is complete; these items are still required to submit the assignment.

### Required

- [ ] **Verify locally** — from the repo root:
  ```bash
  docker compose up --build
  curl http://localhost:8080/health
  cargo test -p invoice-service --test integration
  ```
  Run the curl examples above (customer → invoice → pay success → pay decline on a new invoice).

- [ ] **Record demo video (5–10 min)** — cover, in order:
  1. Architecture overview (services, data model, request flow)
  2. Live demo (`docker compose up`, create customer/invoice, successful payment, declined payment, webhook deliveries)
  3. Invoice state machine walkthrough (unscripted)
  4. One failure-mode walkthrough from [DESIGN.md](./DESIGN.md) section 3 (open the relevant code; `tok_timeout` or `tok_network_error` work well)

- [ ] **Add video link** — replace the placeholder in [Demo Video](#demo-video) with a public Loom, Drive, or S3 link (no login required).

- [ ] **Push to GitHub** — public repo or share access with Dodo. Include source, migrations, `docker-compose.yml`, and all docs.

- [ ] **Review [AI_USAGE.md](./AI_USAGE.md)** — confirm it accurately reflects how you used AI and what you decided yourself.

### Optional polish (not blockers)

- [ ] `GET /v1/invoices/:id` could include the latest payment attempt (mentioned in the plan, not required by the assignment spec).
- [ ] `POST /api-keys` for key rotation is not implemented; the seeded demo key is sufficient for the demo.

### Pre-submit checklist

- [ ] `docker compose up` works on a clean machine with no manual steps
- [ ] Integration tests pass
- [ ] Demo video link is in README and accessible without login
- [ ] No floats in the money path (all amounts are integer cents)
