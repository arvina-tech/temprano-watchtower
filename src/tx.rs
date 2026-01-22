use alloy::consensus::transaction::SignerRecoverable;
use alloy::consensus::{Transaction, TxEnvelope};
use alloy::eips::Decodable2718;
use alloy::eips::eip2718::{EIP4844_TX_TYPE_ID, EIP7702_TX_TYPE_ID};
use alloy::primitives::{Address, B256, U256, keccak256};
use anyhow::{Context, Result};
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
}

pub fn parse_raw_tx(raw_hex: &str) -> Result<ParsedTx> {
    let raw_hex = raw_hex.strip_prefix("0x").unwrap_or(raw_hex);
    let raw_tx = hex::decode(raw_hex).context("decode raw tx hex")?;
    if raw_tx.is_empty() {
        anyhow::bail!("empty raw tx");
    }

    let tx_hash = keccak256(&raw_tx);

    let ty = *raw_tx.first().context("missing tx type")?;
    if ty == TEMPO_TX_TYPE_ID {
        return parse_tempo_tx(raw_tx, tx_hash);
    }

    parse_eip_tx(raw_tx, tx_hash)
}

fn parse_tempo_tx(raw_tx: Vec<u8>, tx_hash: B256) -> Result<ParsedTx> {
    let mut buf = raw_tx.as_slice();
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
    })
}

fn parse_eip_tx(raw_tx: Vec<u8>, tx_hash: B256) -> Result<ParsedTx> {
    let envelope = match TxEnvelope::decode_2718_exact(&raw_tx) {
        Ok(envelope) => envelope,
        Err(alloy::eips::eip2718::Eip2718Error::UnexpectedType(ty)) => {
            anyhow::bail!("unsupported tx type 0x{ty:02x}");
        }
        Err(err) => {
            return Err(anyhow::anyhow!("decode ethereum transaction: {err}"));
        }
    };

    match &envelope {
        TxEnvelope::Eip4844(_) => {
            anyhow::bail!("unsupported tx type 0x{EIP4844_TX_TYPE_ID:02x}");
        }
        TxEnvelope::Eip7702(_) => {
            anyhow::bail!("unsupported tx type 0x{EIP7702_TX_TYPE_ID:02x}");
        }
        _ => {}
    }

    let sender = envelope
        .recover_signer()
        .context("recover sender signature")?;
    let chain_id = envelope.chain_id().context("missing chainId")?;

    Ok(ParsedTx {
        tx_hash,
        sender,
        fee_payer: None,
        chain_id,
        nonce_key: U256::ZERO,
        nonce: envelope.nonce(),
        valid_after: None,
        valid_before: None,
        raw_tx,
    })
}

