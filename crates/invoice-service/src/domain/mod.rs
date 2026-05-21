use serde::{Deserialize, Serialize};
use sqlx::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "invoice_state", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum InvoiceState {
    Draft,
    Open,
    Paid,
    Void,
    Uncollectible,
}

impl InvoiceState {
    pub fn can_pay(self) -> bool {
        matches!(self, Self::Open)
    }

    pub fn can_void(self) -> bool {
        matches!(self, Self::Open)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Paid | Self::Void | Self::Uncollectible)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "payment_attempt_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum PaymentAttemptStatus {
    Processing,
    Pending,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "webhook_delivery_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum WebhookDeliveryStatus {
    Pending,
    Delivered,
    Dead,
}

pub fn compute_total_cents(items: &[(i64, i64)]) -> Result<i64, String> {
    items
        .iter()
        .try_fold(0i64, |acc, (qty, unit)| {
            qty.checked_mul(*unit)
                .and_then(|v| acc.checked_add(v))
                .ok_or_else(|| "total overflow".into())
        })
}
