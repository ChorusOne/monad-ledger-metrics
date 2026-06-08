use std::collections::HashMap;
use std::time::Duration;

use alloy_primitives::hex;
use alloy_sol_types::SolValue;
use serde::{Deserialize, Serialize};

const GET_VALIDATOR_SELECTOR: [u8; 4] = [0x2b, 0x6d, 0x63, 0x9a];
const GET_CONSENSUS_VALSET_SELECTOR: [u8; 4] = [0xfb, 0x29, 0xb7, 0x29];

const MAX_VALSET_PAGES: usize = 1000;

#[derive(Debug)]
pub enum StakingError {
    Http(String),
    Rpc { code: i64, message: String },
    Decode(String),
}

impl std::fmt::Display for StakingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(s) => write!(f, "http error: {s}"),
            Self::Rpc { code, message } => write!(f, "rpc error {code}: {message}"),
            Self::Decode(s) => write!(f, "decode error: {s}"),
        }
    }
}

impl std::error::Error for StakingError {}

#[derive(Serialize)]
struct EthCallObject<'a> {
    to: &'a str,
    data: String,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RpcParam<'a> {
    Call(EthCallObject<'a>),
    Block(&'static str),
}

#[derive(Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    method: &'static str,
    params: [RpcParam<'a>; 2],
    id: u64,
}

#[derive(Deserialize)]
struct JsonRpcResponse {
    result: Option<String>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

fn eth_call(rpc_url: &str, contract: &str, calldata: &[u8]) -> Result<Vec<u8>, StakingError> {
    let req = JsonRpcRequest {
        jsonrpc: "2.0",
        method: "eth_call",
        params: [
            RpcParam::Call(EthCallObject {
                to: contract,
                data: format!("0x{}", hex::encode(calldata)),
            }),
            RpcParam::Block("latest"),
        ],
        id: 1,
    };

    let resp: JsonRpcResponse = ureq::post(rpc_url)
        .timeout(Duration::from_secs(15))
        .send_json(&req)
        .map_err(|e| StakingError::Http(e.to_string()))?
        .into_json()
        .map_err(|e| StakingError::Http(e.to_string()))?;

    if let Some(err) = resp.error {
        return Err(StakingError::Rpc {
            code: err.code,
            message: err.message,
        });
    }

    let result_hex = resp
        .result
        .ok_or_else(|| StakingError::Decode("missing result field".into()))?;

    let stripped = result_hex.strip_prefix("0x").unwrap_or(&result_hex);

    hex::decode(stripped).map_err(|e| StakingError::Decode(e.to_string()))
}

fn fetch_consensus_valset(rpc_url: &str, contract: &str) -> Result<Vec<u64>, StakingError> {
    let mut start_index: u64 = 0;
    let mut all = Vec::new();

    for _ in 0..MAX_VALSET_PAGES {
        let mut calldata = Vec::with_capacity(36);
        calldata.extend_from_slice(&GET_CONSENSUS_VALSET_SELECTOR);
        calldata.extend_from_slice(&(start_index,).abi_encode_params());

        let raw = eth_call(rpc_url, contract, &calldata)?;
        let (is_done, next_index, batch): (bool, u64, Vec<u64>) =
            SolValue::abi_decode_params(&raw, true)
                .map_err(|e| StakingError::Decode(format!("consensus valset: {e}")))?;

        all.extend_from_slice(&batch);

        if is_done {
            return Ok(all);
        }

        if next_index == start_index {
            return Err(StakingError::Decode(
                "consensus valset pagination not advancing".into(),
            ));
        }
        
        start_index = next_index;
    }

    Err(StakingError::Decode(format!(
        "consensus valset exceeded {MAX_VALSET_PAGES} pages"
    )))
}

/// Decode the secp pubkey (index 10, a `bytes`) from a `getValidator` return tuple.
/// The tuple has 12 head slots (32 bytes each); slot 10 holds the offset to the
/// dynamic `bytes` payload, which is `(uint256 length, padded data)`.
fn decode_secp_pubkey(result: &[u8]) -> Result<Vec<u8>, StakingError> {
    const HEAD_SLOTS: usize = 12;
    const SLOT: usize = 32;
    const SECP_HEAD_START: usize = 10 * SLOT;

    if result.len() < HEAD_SLOTS * SLOT {
        return Err(StakingError::Decode(format!(
            "getValidator result too short: {} bytes",
            result.len()
        )));
    }

    let offset_slot = &result[SECP_HEAD_START..SECP_HEAD_START + SLOT];
    let offset = u64::from_be_bytes(offset_slot[24..32].try_into().unwrap()) as usize;
    
    if result.len() < offset + SLOT {
        return Err(StakingError::Decode(
            "secp pubkey offset out of range".into(),
        ));
    }
    
    let len_slot = &result[offset..offset + SLOT];
    let len = u64::from_be_bytes(len_slot[24..32].try_into().unwrap()) as usize;
    let data_start = offset + SLOT;

    if result.len() < data_start + len {
        return Err(StakingError::Decode(
            "secp pubkey data out of range".into(),
        ));
    }

    Ok(result[data_start..data_start + len].to_vec())
}

fn fetch_validator_secp_pubkey(
    rpc_url: &str,
    contract: &str,
    val_id: u64,
) -> Result<String, StakingError> {
    let mut calldata = Vec::with_capacity(36);
    calldata.extend_from_slice(&GET_VALIDATOR_SELECTOR);
    calldata.extend_from_slice(&(val_id,).abi_encode_params());

    let raw = eth_call(rpc_url, contract, &calldata)?;
    let pubkey = decode_secp_pubkey(&raw)?;
    
    Ok(hex::encode(pubkey))
}

/// Returns a map of secp pubkey hex (no `0x` prefix) → validator id for every
/// validator currently in the consensus validator set.
pub fn fetch_consensus_set(
    rpc_url: &str,
    contract: &str,
) -> Result<HashMap<String, u64>, StakingError> {
    let val_ids = fetch_consensus_valset(rpc_url, contract)?;
    let mut out = HashMap::with_capacity(val_ids.len());

    for val_id in val_ids {
        let pubkey = fetch_validator_secp_pubkey(rpc_url, contract, val_id)?;
        out.insert(pubkey, val_id);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_secp_pubkey_extracts_dynamic_bytes() {
        // Synthesize a getValidator return: 12 head slots, with slot 10 pointing
        // at the dynamic data section that follows the heads.
        let mut buf = vec![0u8; 12 * 32];
        let offset = (12 * 32) as u64;
        buf[10 * 32 + 24..10 * 32 + 32].copy_from_slice(&offset.to_be_bytes());

        let pubkey = [0xab; 33];
        let mut len_slot = [0u8; 32];
        len_slot[24..32].copy_from_slice(&(pubkey.len() as u64).to_be_bytes());
        buf.extend_from_slice(&len_slot);
        buf.extend_from_slice(&pubkey);
        // Pad pubkey data to 32-byte boundary, like real ABI encoding.
        buf.resize(buf.len() + (32 - pubkey.len() % 32), 0);

        let decoded = decode_secp_pubkey(&buf).unwrap();
        assert_eq!(decoded, pubkey);
    }
}
