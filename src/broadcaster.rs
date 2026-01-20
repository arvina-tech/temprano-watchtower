use std::time::Duration;

use alloy::providers::Provider;

use crate::rpc::ChainRpc;

#[derive(Debug)]
pub enum BroadcastOutcome {
    Accepted { error: Option<String> },
    Retry { error: String },
    Invalid { error: String },
}

pub async fn broadcast_raw_tx(
    chain: &ChainRpc,
    raw_tx: &[u8],
    fanout: usize,
    timeout: Duration,
    attempt: i32,
) -> BroadcastOutcome {
    if chain.http.is_empty() {
        return BroadcastOutcome::Retry {
            error: "no rpc endpoints".to_string(),
        };
    }

    let total = chain.http.len();
    let fanout = fanout.max(1).min(total);
    let start = (attempt.max(0) as usize) % total;

    let mut errors = Vec::new();
    let mut invalid_errors = Vec::new();
    let mut accepted = false;

    for idx in 0..fanout {
        let provider = chain.http[(start + idx) % total].clone();
        let res = tokio::time::timeout(timeout, provider.send_raw_transaction(raw_tx)).await;
        match res {
            Ok(Ok(_pending)) => {
                accepted = true;
            }
            Ok(Err(err)) => {
                let msg = err.to_string();
                match classify_error(&msg) {
                    ErrorClass::AlreadyKnown => {
                        accepted = true;
                        errors.push(msg);
                    }
                    ErrorClass::Invalid => invalid_errors.push(msg),
                    ErrorClass::Retry => errors.push(msg),
                }
            }
            Err(_elapsed) => {
                errors.push("broadcast timeout".to_string());
            }
        }
    }

    if accepted {
        return BroadcastOutcome::Accepted {
            error: errors.first().cloned(),
        };
    }

    if !invalid_errors.is_empty() {
        return BroadcastOutcome::Invalid {
            error: invalid_errors.join("; "),
        };
    }

    BroadcastOutcome::Retry {
        error: errors.join("; "),
    }
}

#[derive(Debug, Clone, Copy)]
enum ErrorClass {
    AlreadyKnown,
    Invalid,
    Retry,
}

fn classify_error(message: &str) -> ErrorClass {
    let msg = message.to_lowercase();
    if msg.contains("already known")
        || msg.contains("known transaction")
        || msg.contains("already imported")
        || msg.contains("already exists")
    {
        return ErrorClass::AlreadyKnown;
    }

    if msg.contains("invalid")
        || msg.contains("malformed")
        || msg.contains("signature")
        || msg.contains("fee payer")
        || msg.contains("expired")
        || msg.contains("nonce key")
    {
        return ErrorClass::Invalid;
    }

    ErrorClass::Retry
}

#[cfg(test)]
mod tests {
    use super::{ErrorClass, classify_error};

    #[test]
    fn classify_error_handles_known() {
        assert!(matches!(
            classify_error("already known"),
            ErrorClass::AlreadyKnown
        ));
        assert!(matches!(
            classify_error("known transaction"),
            ErrorClass::AlreadyKnown
        ));
    }

    #[test]
    fn classify_error_handles_invalid() {
        assert!(matches!(
            classify_error("invalid signature"),
            ErrorClass::Invalid
        ));
        assert!(matches!(
            classify_error("fee payer signature invalid"),
            ErrorClass::Invalid
        ));
        assert!(matches!(
            classify_error("nonce key invalid"),
            ErrorClass::Invalid
        ));
    }

    #[test]
    fn classify_error_defaults_retry() {
        assert!(matches!(classify_error("timeout"), ErrorClass::Retry));
        assert!(matches!(classify_error("temporary"), ErrorClass::Retry));
    }
}
