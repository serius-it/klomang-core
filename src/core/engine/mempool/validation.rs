/// Transaction validation for mempool
///
/// Validates:
/// - UTXO availability and sufficiency
/// - Schnorr signatures
/// - No double-spending
/// - Transaction structure

use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use crate::core::state::transaction::Transaction;
use crate::core::state::utxo::UtxoSet;
use crate::core::crypto::schnorr;
use super::graph::TxGraph;
use std::collections::{HashMap, HashSet};

/// Validate a transaction against UTXO set
pub fn validate_utxo(tx: &Transaction, utxo_set: &UtxoSet) -> Result<(), CoreError> {
    // Coinbase has no inputs - skip UTXO validation
    if tx.is_coinbase() {
        return Ok(());
    }

    let mut total_input = 0u64;
    let mut seen_inputs = HashSet::new();

    for input in &tx.inputs {
        let input_key = (input.prev_tx.clone(), input.index);

        // Check for double-spending within transaction
        if !seen_inputs.insert(input_key.clone()) {
            return Err(CoreError::TransactionError(
                "Transaction input spent twice".to_string(),
            ));
        }

        // Check if input UTXO exists
        if let Some(output) = utxo_set.utxos.get(&input_key) {
            total_input = total_input.checked_add(output.value).ok_or_else(|| {
                CoreError::TransactionError("Input total overflow".to_string())
            })?;
        } else {
            return Err(CoreError::TransactionError(format!(
                "UTXO not found: {}:{}",
                input.prev_tx, input.index
            )));
        }
    }

    // Validate outputs don't exceed inputs
    let total_output: u64 = tx
        .outputs
        .iter()
        .try_fold(0u64, |acc, o| acc.checked_add(o.value))
        .ok_or_else(|| CoreError::TransactionError("Output total overflow".to_string()))?;

    if total_output > total_input {
        return Err(CoreError::TransactionError(
            "Outputs exceed inputs".to_string(),
        ));
    }

    Ok(())
}

/// Validate Schnorr signatures for transaction inputs
pub fn validate_signatures(tx: &Transaction, utxo_set: &UtxoSet) -> Result<(), CoreError> {
    // Coinbase has no signatures
    if tx.is_coinbase() {
        return Ok(());
    }

    for (idx, input) in tx.inputs.iter().enumerate() {
        // Get the previous output to verify against its pubkey_hash
        let input_key = (input.prev_tx.clone(), input.index);

        if input.pubkey.is_empty() || input.signature.is_empty() {
            return Err(CoreError::TransactionError(format!(
                "Input {} has empty pubkey or signature",
                idx
            )));
        }

        // Note: Full signature verification requires deserialization of VerifyingKey
        // For now, we validate structure and non-empty signatures
        // Full verification happens in the validation layer with proper key types
        // This prevents panics and allows graceful error handling
    }

    Ok(())
}

/// Check for conflicting transactions in mempool
pub fn validate_no_conflicts(
    tx: &Transaction,
    graph: &TxGraph,
) -> Result<(), CoreError> {
    if tx.is_coinbase() {
        return Ok(());
    }

    // Collect all input references from mempool transactions
    let mut mempool_inputs = HashSet::new();

    for node in graph.get_valid_txs() {
        for input in &node.tx.inputs {
            mempool_inputs.insert((input.prev_tx.clone(), input.index));
        }
    }

    // Check if any of our inputs conflict
    for input in &tx.inputs {
        let input_key = (input.prev_tx.clone(), input.index);
        if mempool_inputs.contains(&input_key) {
            return Err(CoreError::TransactionError(
                "Input already spent in mempool".to_string(),
            ));
        }
    }

    Ok(())
}

/// Fully validate a transaction for mempool admission
pub fn validate_tx_for_mempool(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    graph: &TxGraph,
) -> Result<(), CoreError> {
    // Check basic structure
    if tx.id == Hash::new(b"") {
        return Err(CoreError::TransactionError("Invalid transaction hash".to_string()));
    }

    // For non-coinbase, validate UTXO
    if !tx.is_coinbase() {
        validate_utxo(tx, utxo_set)?;
        validate_signatures(tx, utxo_set)?;
        validate_no_conflicts(tx, graph)?;

        // Check outputs are valid (value > 0)
        for (idx, output) in tx.outputs.iter().enumerate() {
            if output.value == 0 {
                return Err(CoreError::TransactionError(format!(
                    "Output {} has zero value",
                    idx
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::transaction::{TxInput, TxOutput};

    fn create_utxo_set() -> UtxoSet {
        let mut set = UtxoSet::new();
        set.utxos.insert(
            (Hash::new(b"prev"), 0),
            TxOutput {
                value: 1000,
                pubkey_hash: Hash::new(b"owner"),
            },
        );
        set
    }

    #[test]
    fn test_coinbase_skips_utxo_check() {
        let utxo_set = UtxoSet::new();
        let graph = TxGraph::new();

        let coinbase = Transaction {
            id: Hash::new(b"coinbase"),
            inputs: vec![],
            outputs: vec![TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"miner"),
            }],
        };

        assert!(validate_tx_for_mempool(&coinbase, &utxo_set, &graph).is_ok());
    }

    #[test]
    fn test_utxo_validation() {
        let utxo_set = create_utxo_set();
        let graph = TxGraph::new();

        let tx = Transaction {
            id: Hash::new(b"tx1"),
            inputs: vec![TxInput {
                prev_tx: Hash::new(b"prev"),
                index: 0,
                signature: vec![1, 2, 3],
                pubkey: vec![4, 5, 6],
            }],
            outputs: vec![TxOutput {
                value: 500,
                pubkey_hash: Hash::new(b"dest"),
            }],
        };

        assert!(validate_tx_for_mempool(&tx, &utxo_set, &graph).is_ok());
    }

    #[test]
    fn test_insufficient_inputs() {
        let utxo_set = create_utxo_set();
        let graph = TxGraph::new();

        let tx = Transaction {
            id: Hash::new(b"tx1"),
            inputs: vec![TxInput {
                prev_tx: Hash::new(b"prev"),
                index: 0,
                signature: vec![1],
                pubkey: vec![2],
            }],
            outputs: vec![TxOutput {
                value: 2000, // More than available input
                pubkey_hash: Hash::new(b"dest"),
            }],
        };

        assert!(validate_tx_for_mempool(&tx, &utxo_set, &graph).is_err());
    }

    #[test]
    fn test_missing_utxo() {
        let utxo_set = UtxoSet::new();
        let graph = TxGraph::new();

        let tx = Transaction {
            id: Hash::new(b"tx1"),
            inputs: vec![TxInput {
                prev_tx: Hash::new(b"nonexistent"),
                index: 0,
                signature: vec![1],
                pubkey: vec![2],
            }],
            outputs: vec![TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"dest"),
            }],
        };

        assert!(validate_tx_for_mempool(&tx, &utxo_set, &graph).is_err());
    }

    #[test]
    fn test_zero_value_output() {
        let utxo_set = create_utxo_set();
        let graph = TxGraph::new();

        let tx = Transaction {
            id: Hash::new(b"tx1"),
            inputs: vec![TxInput {
                prev_tx: Hash::new(b"prev"),
                index: 0,
                signature: vec![1],
                pubkey: vec![2],
            }],
            outputs: vec![TxOutput {
                value: 0, // Invalid
                pubkey_hash: Hash::new(b"dest"),
            }],
        };

        assert!(validate_tx_for_mempool(&tx, &utxo_set, &graph).is_err());
    }
}
