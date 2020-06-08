//! Deposit
//!
//! Deposit coins into state entity

// deposit() messages:
// 0. Initiate session - generate ID and perform authorisation
// 1. Generate shared wallet
// 2. user sends backup tx data
// 3. Co-op sign back-up tx

use super::super::Result;
extern crate shared_lib;
use shared_lib::util::{FEE,tx_funding_build, tx_backup_build};
use shared_lib::structs::{DepositMsg1, Protocol, PrepareSignTxMsg};

use crate::wallet::wallet::{to_bitcoin_public_key,Wallet};
use crate::utilities::requests;
use crate::state_entity::util::{cosign_tx_input,verify_statechain_smt};
use crate::error::{WalletErrorType, CError};
use super::api::{get_smt_proof, get_smt_root, get_statechain_fee_info};

use bitcoin::{Transaction, PublicKey};
use curv::elliptic::curves::traits::ECPoint;


/// Message to server initiating state entity protocol.
/// Shared wallet ID returned
pub fn session_init(wallet: &mut Wallet, proof_key: &String) -> Result<String> {
    requests::postb(&wallet.client_shim,&format!("/deposit/init"),
        &DepositMsg1 {
            auth: "auth".to_string(),
            proof_key: proof_key.to_owned()
        }
    )
}

/// Deposit coins into state entity. Returns shared_key_id, state_chain_id, signed funding tx,
/// signed backup tx, back up transacion data and proof_key
pub fn deposit(wallet: &mut Wallet, amount: &u64)
    -> Result<(String, String, Transaction, Transaction, PrepareSignTxMsg, PublicKey)>
{
    // get state entity fee info
    let se_fee_info = get_statechain_fee_info(&wallet.client_shim)?;

    // Ensure funds cover fees before initiating protocol
    if FEE+se_fee_info.deposit >= *amount {
        return Err(CError::WalletError(WalletErrorType::NotEnoughFunds));
    }

    // Greedy coin selection.
    let (inputs, addrs, amounts) = wallet.coin_selection_greedy(&(amount+se_fee_info.deposit+FEE))?;

    // generate proof key
    let proof_key = wallet.se_proof_keys.get_new_key()?;

    // init. session - Receive shared wallet ID
    let shared_key_id: String = session_init(wallet, &proof_key.to_string())?;

    // 2P-ECDSA with state entity to create a Shared key
    let shared_key = wallet.gen_shared_key(&shared_key_id, amount)?;

    // make funding tx
    let pk = shared_key.share.public.q.get_element();   // co-owned key address to send funds to (P_addr)
    let p_addr = bitcoin::Address::p2wpkh(
        &to_bitcoin_public_key(pk),
        wallet.get_bitcoin_network()
    );

    let change_addr = wallet.keys.get_new_address()?.to_string();
    let change_amount = amounts.iter().sum::<u64>() - amount - se_fee_info.deposit - FEE;
    let tx_0 = tx_funding_build(&inputs, &p_addr.to_string(), amount, &se_fee_info.deposit, &se_fee_info.address, &change_addr, &change_amount)?;
    let tx_funding_signed = wallet.sign_tx(
        &tx_0,
        &(0..inputs.len()).collect(), // inputs to sign are all inputs is this case
        &addrs,
        &amounts
    );

    // Make unsigned backup tx
    let backup_receive_addr = wallet.se_backup_keys.get_new_address()?;
    let tx_backup_unsigned = tx_backup_build(
        &tx_funding_signed.txid(),
        &backup_receive_addr,
        &amount
    )?;

    let tx_backup_psm = PrepareSignTxMsg {
        protocol: Protocol::Deposit,
        tx: tx_backup_unsigned.to_owned(),
        input_addrs: vec!(p_addr.to_string()),
        input_amounts: vec!(*amount),
        proof_key: Some(proof_key.to_string()),
    };

    // co-sign tx backup tx
    let (witness, state_chain_id) = cosign_tx_input(wallet, &shared_key_id, &tx_backup_psm)?;
    // add witness to back up tx
    let mut tx_backup_signed = tx_backup_unsigned.clone();
    tx_backup_signed.input[0].witness = witness;

    // TODO: Broadcast funding transcation

    // verify proof key inclusion in SE sparse merkle tree
    let root = get_smt_root(wallet)?;
    let proof = get_smt_proof(wallet, &root, &tx_funding_signed.txid().to_string())?;
    assert!(verify_statechain_smt(
        &root.value,
        &proof_key.to_string(),
        &proof
    ));

    // Add proof and other data to Shared key
    {
        let shared_key = wallet.get_shared_key_mut(&shared_key_id)?;
        shared_key.state_chain_id = Some(state_chain_id.to_string());
        shared_key.tx_backup_psm = Some(tx_backup_psm.to_owned());
        shared_key.add_proof_data(&proof_key.to_string(), &root, &proof);
    }

    Ok((shared_key_id, state_chain_id, tx_funding_signed, tx_backup_signed, tx_backup_psm, proof_key))
}
