CREATE TYPE invoice_state AS ENUM ('draft', 'open', 'paid', 'void', 'uncollectible');
CREATE TYPE payment_attempt_status AS ENUM ('processing', 'pending', 'succeeded', 'failed');
CREATE TYPE webhook_delivery_status AS ENUM ('pending', 'delivered', 'dead');

CREATE TABLE businesses (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE api_keys (
    id UUID PRIMARY KEY,
    business_id UUID NOT NULL REFERENCES businesses(id),
    key_prefix TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_api_keys_prefix ON api_keys(key_prefix) WHERE revoked_at IS NULL;

CREATE TABLE customers (
    id UUID PRIMARY KEY,
    business_id UUID NOT NULL REFERENCES businesses(id),
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_customers_business ON customers(business_id);

CREATE TABLE invoices (
    id UUID PRIMARY KEY,
    business_id UUID NOT NULL REFERENCES businesses(id),
    customer_id UUID NOT NULL REFERENCES customers(id),
    total_cents BIGINT NOT NULL CHECK (total_cents >= 0),
    state invoice_state NOT NULL,
    due_date DATE NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_invoices_business_state ON invoices(business_id, state);
CREATE INDEX idx_invoices_customer ON invoices(customer_id);

CREATE TABLE invoice_line_items (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    description TEXT NOT NULL,
    quantity BIGINT NOT NULL CHECK (quantity > 0),
    unit_amount_cents BIGINT NOT NULL CHECK (unit_amount_cents >= 0)
);

CREATE INDEX idx_line_items_invoice ON invoice_line_items(invoice_id);

CREATE TABLE payment_attempts (
    id UUID PRIMARY KEY,
    business_id UUID NOT NULL REFERENCES businesses(id),
    invoice_id UUID NOT NULL REFERENCES invoices(id),
    idempotency_key TEXT NOT NULL,
    request_body_hash TEXT NOT NULL,
    card_token TEXT NOT NULL,
    status payment_attempt_status NOT NULL,
    psp_ref TEXT,
    failure_code TEXT,
    response_status_code INT,
    response_body JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (business_id, idempotency_key)
);

CREATE INDEX idx_payment_attempts_invoice ON payment_attempts(invoice_id);
CREATE INDEX idx_payment_attempts_pending ON payment_attempts(status) WHERE status IN ('pending', 'processing');

CREATE TABLE webhook_endpoints (
    id UUID PRIMARY KEY,
    business_id UUID NOT NULL REFERENCES businesses(id),
    url TEXT NOT NULL,
    signing_secret TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_webhook_endpoints_business ON webhook_endpoints(business_id);

CREATE TABLE webhook_deliveries (
    id UUID PRIMARY KEY,
    business_id UUID NOT NULL REFERENCES businesses(id),
    endpoint_id UUID NOT NULL REFERENCES webhook_endpoints(id),
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    status webhook_delivery_status NOT NULL DEFAULT 'pending',
    attempt_count INT NOT NULL DEFAULT 0,
    next_retry_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_webhook_deliveries_pending ON webhook_deliveries(status, next_retry_at) WHERE status = 'pending';

-- Seed demo business and API key (dodo_live_demokey1234567890123456789012)
INSERT INTO businesses (id, name) VALUES ('11111111-1111-1111-1111-111111111111', 'Demo Business');

-- SHA-256 of dodo_live_demokey1234567890123456789012
INSERT INTO api_keys (id, business_id, key_prefix, key_hash) VALUES (
    '22222222-2222-2222-2222-222222222222',
    '11111111-1111-1111-1111-111111111111',
    'dodo_liv',
    'e12592912fa32521729811a474501365d8b5bc40f2282016c04c79fbc5ffaea0'
);
