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
}

const GROUP_MAGIC: [u8; 4] = *b"TWGR";
const GROUP_TYPE: [u8; 2] = [0x00, 0x01];

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

    let group = extract_group_memo(&tx.calls);

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

fn extract_group_memo(calls: &[tempo_alloy::primitives::transaction::Call]) -> Option<GroupMemo> {
    for call in calls {
        if let Ok(decoded) = ITIP20::transferWithMemoCall::abi_decode(call.input.as_ref()) {
            let memo_bytes = decoded.memo.as_slice();
            if memo_bytes.len() != 32 {
                continue;
            }
            let mut memo = [0u8; 32];
            memo.copy_from_slice(memo_bytes);
            if let Some(group) = parse_group_memo(&memo) {
                return Some(group);
            }
        }
    }

    None
}

fn parse_group_memo(memo: &[u8; 32]) -> Option<GroupMemo> {
    if memo[0..4] != GROUP_MAGIC {
        return None;
    }
    if memo[6..8] != GROUP_TYPE {
        return None;
    }

    let version = memo[4];
    let mut group_id = [0u8; 16];
    let mut aux = [0u8; 8];
    group_id.copy_from_slice(&memo[8..24]);
    aux.copy_from_slice(&memo[24..32]);

    Some(GroupMemo {
        group_id,
        aux,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::{GROUP_MAGIC, GROUP_TYPE, parse_group_memo};

    #[test]
    fn parse_group_memo_accepts_valid() {
        let mut memo = [0u8; 32];
        memo[0..4].copy_from_slice(&GROUP_MAGIC);
        memo[4] = 0x01;
        memo[6..8].copy_from_slice(&GROUP_TYPE);
        memo[8..24].copy_from_slice(&[0x11; 16]);
        memo[24..32].copy_from_slice(&[0x22; 8]);

        let parsed = parse_group_memo(&memo).expect("group memo parsed");
        assert_eq!(parsed.version, 0x01);
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
}
