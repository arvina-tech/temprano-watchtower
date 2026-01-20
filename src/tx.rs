use alloy::consensus::transaction::SignerRecoverable;
use alloy::primitives::{Address, B256, U256, keccak256};
use alloy::sol_types::SolCall;
use anyhow::{Context, Result};
use tempo_alloy::contracts::precompiles::ITIP20;
use tempo_alloy::primitives::transaction::{AASigned, TEMPO_TX_TYPE_ID, validate_calls};

pub struct ParsedTx {
    pub tx_hash: B256,
    pub sender: Address,
    pub fee_payer: Option<Address>,
    pub chain_id: u64,
    pub nonce_key: U256,
    pub nonce: u64,
    pub valid_after: Option<u64>,
    pub valid_before: Option<u64>,
    pub raw_tx: Vec<u8>,
    pub group: Option<GroupMemo>,
}

#[derive(Debug, Clone)]
pub struct GroupMemo {
    pub group_id: [u8; 16],
    pub aux: [u8; 8],
    pub version: u8,
    pub flags: u8,
}

const GROUP_MAGIC: [u8; 4] = *b"TWGR";
const GROUP_TYPE: [u8; 2] = [0x00, 0x01];
const GROUP_VERSION: u8 = 0x01;

pub fn parse_raw_tx(raw_hex: &str) -> Result<ParsedTx> {
    let raw_hex = raw_hex.strip_prefix("0x").unwrap_or(raw_hex);
    let raw_tx = hex::decode(raw_hex).context("decode raw tx hex")?;
    if raw_tx.is_empty() {
        anyhow::bail!("empty raw tx");
    }

    let tx_hash = keccak256(&raw_tx);

    let mut buf = raw_tx.as_slice();
    let ty = *buf.first().context("missing tx type")?;
    if ty != TEMPO_TX_TYPE_ID {
        anyhow::bail!("unsupported tx type 0x{ty:02x}");
    }
    buf = &buf[1..];

    let signed = AASigned::rlp_decode(&mut buf).context("decode tempo transaction")?;
    if !buf.is_empty() {
        anyhow::bail!("trailing bytes after decoding tempo transaction");
    }

    let sender = signed
        .recover_signer()
        .context("recover sender signature")?;

    let tx = signed.tx();
    validate_calls(&tx.calls, !tx.tempo_authorization_list.is_empty())
        .map_err(|err| anyhow::anyhow!(err))?;

    let fee_payer = if tx.fee_payer_signature.is_some() {
        Some(
            tx.recover_fee_payer(sender)
                .context("recover fee payer signature")?,
        )
    } else {
        None
    };

    let group = extract_group_memo(&tx.calls)?;

    Ok(ParsedTx {
        tx_hash,
        sender,
        fee_payer,
        chain_id: tx.chain_id,
        nonce_key: tx.nonce_key,
        nonce: tx.nonce,
        valid_after: tx.valid_after,
        valid_before: tx.valid_before,
        raw_tx,
        group,
    })
}

fn extract_group_memo(
    calls: &[tempo_alloy::primitives::transaction::Call],
) -> Result<Option<GroupMemo>> {
    let mut group_ids = std::collections::BTreeSet::new();
    let mut first_group = None;
    for call in calls {
        if let Some(memo) = tip20_memo(call.input.as_ref())
            && let Some(group) = parse_group_memo(&memo)
        {
            group_ids.insert(group.group_id);
            if first_group.is_none() {
                first_group = Some(group);
            }
        }

        if group_ids.len() > 1 {
            anyhow::bail!("transaction has more than one memo call with different groups");
        }
    }

    Ok(first_group)
}

fn tip20_memo(input: &[u8]) -> Option<[u8; 32]> {
    if let Ok(decoded) = ITIP20::transferWithMemoCall::abi_decode(input) {
        return Some(b256_to_bytes(decoded.memo));
    }
    if let Ok(decoded) = ITIP20::transferFromWithMemoCall::abi_decode(input) {
        return Some(b256_to_bytes(decoded.memo));
    }
    None
}

fn b256_to_bytes(value: alloy::primitives::B256) -> [u8; 32] {
    let mut memo = [0u8; 32];
    memo.copy_from_slice(value.as_slice());
    memo
}

fn parse_group_memo(memo: &[u8; 32]) -> Option<GroupMemo> {
    if memo[0..4] != GROUP_MAGIC {
        return None;
    }
    if memo[4] != GROUP_VERSION {
        return None;
    }
    if memo[6..8] != GROUP_TYPE {
        return None;
    }

    let version = memo[4];
    let flags = memo[5];
    let mut group_id = [0u8; 16];
    let mut aux = [0u8; 8];
    group_id.copy_from_slice(&memo[8..24]);
    aux.copy_from_slice(&memo[24..32]);

    Some(GroupMemo {
        group_id,
        aux,
        version,
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        GROUP_MAGIC, GROUP_TYPE, GROUP_VERSION, ITIP20, extract_group_memo, parse_group_memo,
    };
    use alloy::primitives::{Address, B256, Bytes, TxKind, U256};
    use alloy::sol_types::SolCall;
    use tempo_alloy::primitives::transaction::Call;

    #[test]
    fn parse_group_memo_accepts_valid() {
        let mut memo = [0u8; 32];
        memo[0..4].copy_from_slice(&GROUP_MAGIC);
        memo[4] = GROUP_VERSION;
        memo[5] = 0x03;
        memo[6..8].copy_from_slice(&GROUP_TYPE);
        memo[8..24].copy_from_slice(&[0x11; 16]);
        memo[24..32].copy_from_slice(&[0x22; 8]);

        let parsed = parse_group_memo(&memo).expect("group memo parsed");
        assert_eq!(parsed.version, 0x01);
        assert_eq!(parsed.flags, 0x03);
        assert_eq!(parsed.group_id, [0x11; 16]);
        assert_eq!(parsed.aux, [0x22; 8]);
    }

    #[test]
    fn parse_group_memo_rejects_bad_magic() {
        let mut memo = [0u8; 32];
        memo[0..4].copy_from_slice(b"NOPE");
        memo[6..8].copy_from_slice(&GROUP_TYPE);
        assert!(parse_group_memo(&memo).is_none());
    }

    #[test]
    fn parse_group_memo_rejects_unknown_version() {
        let mut memo = [0u8; 32];
        memo[0..4].copy_from_slice(&GROUP_MAGIC);
        memo[4] = GROUP_VERSION + 1;
        memo[6..8].copy_from_slice(&GROUP_TYPE);
        assert!(parse_group_memo(&memo).is_none());
    }

    #[test]
    fn extract_group_memo_rejects_multiple_groups_over_one_call() {
        let memo_a = build_group_memo([0x11; 16], [0x22; 8], 0x00);
        let memo_b = build_group_memo([0x33; 16], [0x44; 8], 0x00);
        let calls = vec![memo_call(memo_a), memo_call(memo_a), memo_call(memo_b)];

        let err = extract_group_memo(&calls).unwrap_err();
        assert!(err.to_string().contains("more than one memo call"));
    }

    #[test]
    fn extract_group_memo_accepts_transfer_from_with_memo() {
        let memo = build_group_memo([0x55; 16], [0x66; 8], 0x00);
        let calls = vec![memo_from_call(memo)];

        let group = extract_group_memo(&calls)
            .expect("extract group memo")
            .expect("group memo present");
        assert_eq!(group.group_id, [0x55; 16]);
        assert_eq!(group.aux, [0x66; 8]);
    }

    #[test]
    fn extract_group_memo_ignores_non_memo_transfers() {
        let calls = vec![transfer_call(), transfer_from_call()];
        let group = extract_group_memo(&calls).expect("extract group memo");
        assert!(group.is_none());
    }

    fn build_group_memo(group_id: [u8; 16], aux: [u8; 8], flags: u8) -> [u8; 32] {
        let mut memo = [0u8; 32];
        memo[0..4].copy_from_slice(&GROUP_MAGIC);
        memo[4] = GROUP_VERSION;
        memo[5] = flags;
        memo[6..8].copy_from_slice(&GROUP_TYPE);
        memo[8..24].copy_from_slice(&group_id);
        memo[24..32].copy_from_slice(&aux);
        memo
    }

    fn memo_call(memo: [u8; 32]) -> Call {
        let transfer_call = ITIP20::transferWithMemoCall {
            to: Address::ZERO,
            amount: U256::from(1u64),
            memo: B256::from(memo),
        };

        Call {
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Bytes::from(transfer_call.abi_encode()),
        }
    }

    fn memo_from_call(memo: [u8; 32]) -> Call {
        let transfer_call = ITIP20::transferFromWithMemoCall {
            from: Address::ZERO,
            to: Address::ZERO,
            amount: U256::from(1u64),
            memo: B256::from(memo),
        };

        Call {
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Bytes::from(transfer_call.abi_encode()),
        }
    }

    fn transfer_call() -> Call {
        let transfer_call = ITIP20::transferCall {
            to: Address::ZERO,
            amount: U256::from(1u64),
        };

        Call {
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Bytes::from(transfer_call.abi_encode()),
        }
    }

    fn transfer_from_call() -> Call {
        let transfer_call = ITIP20::transferFromCall {
            from: Address::ZERO,
            to: Address::ZERO,
            amount: U256::from(1u64),
        };

        Call {
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Bytes::from(transfer_call.abi_encode()),
        }
    }
}
