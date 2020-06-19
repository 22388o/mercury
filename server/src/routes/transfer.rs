//! StateEntity Transfer
//!
//! StateEntity Transfer and batch-transfer protocols.

use super::super::{{Result,Config},
    auth::jwt::Claims,
    storage::db};

extern crate shared_lib;
use shared_lib::{
    structs::*,
    state_chain::*,
    Root, commitment::verify_commitment};
use crate::routes::util::*;
use crate::error::{SEError,DBErrorType::NoDataForID};
use crate::routes::ecdsa;
use crate::storage::db::get_current_root;

use multi_party_ecdsa::protocols::two_party_ecdsa::lindell_2017::party_one::Party1Private;

use curv::{
    elliptic::curves::traits::{ECScalar,ECPoint},
    {BigInt,FE,GE}};
use rocket_contrib::json::Json;
use rocket::State;
use uuid::Uuid;
use db::{DB_SC_LOC, update_root};
use std::{time::SystemTime,
    collections::HashMap};


/// Initiliase transfer protocol:
///     - Authorisation of Owner and DoS protection
///     - Validate transfer parameters
///     - Store transfer parameters
#[post("/transfer/sender", format = "json", data = "<transfer_msg1>")]
pub fn transfer_sender(
    state: State<Config>,
    claim: Claims,
    transfer_msg1: Json<TransferMsg1>,
) -> Result<Json<TransferMsg2>> {
    // Auth user
    check_user_auth(&state, &claim, &transfer_msg1.shared_key_id)?;

    // Verification/PoW/authoriation failed
    // Err(SEError::AuthError)

    // Ensure transfer has not already been completed (but not finalized)
    if db::get::<TransferData>(&state.db, &claim.sub, &transfer_msg1.shared_key_id, &StateEntityStruct::TransferData)?.is_some() {
        return Err(SEError::Generic(String::from("Transfer already completed.")));
    }

    // Get state_chain id
    let user_session: UserSession =
        db::get(&state.db, &claim.sub, &transfer_msg1.shared_key_id, &StateEntityStruct::UserSession)?
            .ok_or(SEError::DBError(NoDataForID, transfer_msg1.shared_key_id.clone()))?;
    if user_session.state_chain_id.is_none() {
        return Err(SEError::Generic(String::from("Transfer Error: User does not own a state chain.")));
    }

    // Generate x1
    let x1: FE = ECScalar::new_random();

    // Create TransferData DB entry
    db::insert(
        &state.db,
        &claim.sub,
        &transfer_msg1.shared_key_id,
        &StateEntityStruct::TransferData,
        &TransferData {
            state_chain_id: user_session.state_chain_id.clone().unwrap(),
            state_chain_sig: transfer_msg1.state_chain_sig.to_owned(),
            x1
        }
    )?;

    debug!("Transfer: Sender side complete. For State Chain: {}",user_session.state_chain_id.unwrap());

    // TODO encrypt x1 with Senders proof key

    Ok(Json(TransferMsg2{x1}))
}

/// Transfer shared wallet to new Owner:
///     - Check new Owner's state chain is correct
///     - Perform 2P-ECDSA key rotation
///     - Return new public shared key S2
#[post("/transfer/receiver", format = "json", data = "<transfer_msg4>")]
pub fn transfer_receiver(
    state: State<Config>,
    claim: Claims,
    transfer_msg4: Json<TransferMsg4>,
) -> Result<Json<TransferMsg5>> {
    let id = transfer_msg4.shared_key_id.clone();
    // Get TransferData for shared_key_id
    let transfer_data: TransferData =
        db::get(&state.db, &claim.sub, &id, &StateEntityStruct::TransferData)?
            .ok_or(SEError::DBError(NoDataForID, id.clone()))?;
    let state_chain_sig = transfer_data.state_chain_sig;

    // Ensure state_chain_sigs are the same
    if state_chain_sig != transfer_msg4.state_chain_sig.to_owned() {
        return Err(SEError::Generic(format!("State chain siganture provided does not match state chain at id {}",transfer_data.state_chain_id)));
    }

    // Get Party1 (State Entity) private share
    let party_1_private: Party1Private = db::get(&state.db, &claim.sub, &id, &ecdsa::EcdsaStruct::Party1Private)?
    .ok_or(SEError::DBError(NoDataForID, id.clone()))?;
    // Get Party2 (Owner 1) public share
    let party_2_public: GE = db::get(&state.db, &claim.sub, &id, &ecdsa::EcdsaStruct::Party2Public)?
        .ok_or(SEError::DBError(NoDataForID, id.clone()))?;

    // TODO: decrypt t2

    let x1 = transfer_data.x1;
    let t2 = transfer_msg4.t2;
    let s1 = party_1_private.get_private_key();

    // Note:
    //  s2 = o1*o2_inv*s1
    //  t2 = o1*x1*o2_inv
    let s2 = t2 * (x1.invert()) * s1;

    // Check s2 is valid for Lindell protocol (s2<q/3)
    let sk_bigint = s2.to_big_int();
    let q_third = FE::q();
    if sk_bigint >= q_third.div_floor(&BigInt::from(3)) {
        return Err(SEError::Generic(format!("Invalid o2, try again.")));
    }

    let g: GE = ECPoint::generator();
    let s2_pub: GE = g * s2;

    let p1_pub = party_2_public * s1;
    let p2_pub = transfer_msg4.o2_pub * s2;

    // Check P1 = o1_pub*s1 === p2 = o2_pub*s2
    if p1_pub != p2_pub {
        debug!("Transfer protocol failed. P1 != P2.");
        return Err(SEError::Generic(String::from("Transfer protocol error: P1 != P2")));
    }

    // Create user ID for new UserSession (receiver of transfer)
    let new_shared_key_id = Uuid::new_v4().to_string();

    let state_chain_id = transfer_data.state_chain_id.clone();
    let finalized_data = TransferFinalizeData {
        new_shared_key_id: new_shared_key_id.clone(),
        state_chain_id: state_chain_id.clone(),
        state_chain_sig,
        s2,
        batch_data: transfer_msg4.batch_data.clone()
    };

    // If batch transfer then mark StateChain as complete and store finalized data in TransferBatchData.
    // This is so the transfers can be finalized when all transfers in the batch are complete.
    if transfer_msg4.batch_data.is_some() {
        let batch_id = transfer_msg4.batch_data.clone().unwrap().id;
        let mut transfer_batch_data: TransferBatchData =
            db::get(&state.db, &claim.sub, &batch_id, &StateEntityStruct::TransferBatchData)?
                .ok_or(SEError::DBError(NoDataForID, batch_id.clone()))?;

        // Ensure batch transfer is still active
        if transfer_batch_data.is_ended(&state.batch_lifetime) {
            return Err(SEError::Generic(String::from("Transfer batch ended. Too late to complete transfer.")));
        }

        transfer_batch_data.state_chains.insert(state_chain_id, true);
        transfer_batch_data.finalized_data.push(finalized_data);
        db::insert(
            &state.db,
            &claim.sub,
            &batch_id,
            &StateEntityStruct::TransferBatchData,
            &transfer_batch_data
        )?;
    // If not batch then finalize transfer now
    } else {
        // Update DB and SMT with new transfer data
        transfer_finalize(
            &state,
            &claim,
            &finalized_data
        )?;
    }

    debug!("Transfer: Receiver side complete and DB updated. State Chain ID: {}",transfer_data.state_chain_id);

    Ok(Json(
        TransferMsg5 {
            new_shared_key_id,
            s2_pub,
        }
    ))
}


/// Update DB and SMT after successful transfer.
/// This function is called immediately in the regular transfer case or after confirmation of atomic
/// transfers completion in the batch transfer case.
pub fn transfer_finalize(
    state: &State<Config>,
    claim: &Claims,
    finalized_data: &TransferFinalizeData
) -> Result<()> {
    // Update state chain
    let mut state_chain: StateChain =
        db::get(&state.db, &claim.sub, &finalized_data.state_chain_id, &StateEntityStruct::StateChain)?
            .ok_or(SEError::DBError(NoDataForID, finalized_data.state_chain_id.clone()))?;

    state_chain.add(finalized_data.state_chain_sig.to_owned())?;

    db::insert(
        &state.db,
        &claim.sub,
        &finalized_data.state_chain_id,
        &StateEntityStruct::StateChain,
        &state_chain
    )?;

    // Create new UserSession to allow new owner to generate shared wallet
    db::insert(
        &state.db,
        &claim.sub,
        &finalized_data.new_shared_key_id,
        &StateEntityStruct::UserSession,
        &UserSession {
            id: finalized_data.new_shared_key_id.clone(),
            auth: String::from("auth"),
            proof_key: finalized_data.state_chain_sig.data.to_owned(),
            tx_backup: Some(state_chain.tx_backup.clone()),
            tx_withdraw: None,
            sig_hash: None,
            state_chain_id: Some(finalized_data.state_chain_id.to_owned()),
            s2: Some(finalized_data.s2),
            withdraw_sc_sig: None
        }
    )?;

    // Update sparse merkle tree with new StateChain entry
    let funding_txid = state_chain.tx_backup.input.get(0).unwrap().previous_output.txid.to_string();
    let proof_key = state_chain.chain.last()
        .ok_or(SEError::Generic(String::from("StateChain empty")))?
        .data.clone();

    let root = get_current_root::<Root>(&state.db)?;
    let new_root = update_statechain_smt(DB_SC_LOC, &root.value, &funding_txid, &proof_key)?;
    update_root(&state.db, new_root.unwrap())?;

    Ok(())
}


/// Request setup of a batch transfer.
///     - Verify all signatures
///     - Create TransferBatchData DB object
#[post("/transfer/batch/init", format = "json", data = "<transfer_batch_init_msg>")]
pub fn transfer_batch_init(
    state: State<Config>,
    claim: Claims,
    transfer_batch_init_msg: Json<TransferBatchInitMsg>
) -> Result<Json<()>> {
    // Ensure sigs purpose is for batch transfer
    for sig in &transfer_batch_init_msg.signatures {
        if !sig.purpose.contains("TRANSFER_BATCH") {
            return Err(SEError::Generic(String::from("Signture's purpose is not valid for batch transfer.")));
        }
    }

    let mut state_chains = HashMap::new();
    let batch_id = transfer_batch_init_msg.id.clone();
    for sig in transfer_batch_init_msg.signatures.clone() {
        // Ensure sig is for same batch as others
        if &sig.clone().purpose[15..] != batch_id {
            return Err(SEError::Generic(String::from("Batch id is not identical for all signtures.")));
        }

        // Verify sig
        let state_chain: StateChain =
            db::get(&state.db, &claim.sub, &sig.data, &StateEntityStruct::StateChain)?
                .ok_or(SEError::DBError(NoDataForID, sig.data.clone()))?;
        let proof_key = state_chain.get_tip()?.data;
        sig.verify(&proof_key)?;

        // Add to TransferBatchData object
        state_chains.insert(
            sig.data,
            false
        );
    }

    // Create new TransferBatchData and add to DB
    db::insert(
        &state.db,
        &claim.sub,
        &batch_id,
        &StateEntityStruct::TransferBatchData,
        &TransferBatchData {
            id: batch_id.clone(),
            start_time: SystemTime::now(),
            state_chains,
            finalized_data: vec!(),
            punished_state_chains: vec!()
        }
    )?;

    Ok(Json(()))
}

/// Finalize all transfers in a batch.
pub fn finalize_batch(
    state: &State<Config>,
    claim: &Claims,
    batch_id: &String
) -> Result<()> {
    let transfer_batch_data: TransferBatchData =
        db::get(&state.db, &claim.sub, &batch_id, &StateEntityStruct::TransferBatchData)?
            .ok_or(SEError::DBError(NoDataForID, batch_id.clone()))?;

    if transfer_batch_data.state_chains.len() != transfer_batch_data.finalized_data.len() {
        return Err(SEError::Generic(String::from("TransferBatchData has unequal finalized data to state chains.")))
    }

    for finalized_data in transfer_batch_data.finalized_data {
        transfer_finalize(
            &state,
            &claim,
            &finalized_data
        )?;
    }

    Ok(())
}


/// API: Reveal a nonce for a corresponding commitment
#[post("/info/transfer/batch/reveal", format = "json", data = "<transfer_reveal_nonce>")]
pub fn transfer_reveal_nonce(
    state: State<Config>,
    claim: Claims,
    transfer_reveal_nonce: Json<TransferRevealNonce>,
) -> Result<Json<()>> {
    let mut transfer_batch_data: TransferBatchData =
        db::get(&state.db, &claim.sub, &transfer_reveal_nonce.batch_id, &StateEntityStruct::TransferBatchData)?
            .ok_or(SEError::DBError(NoDataForID, transfer_reveal_nonce.batch_id.clone()))?;

    if !transfer_batch_data.is_ended(&state.batch_lifetime) {
        return Err(SEError::Generic(String::from("Transfer Batch still live.")));
    }
    if transfer_batch_data.state_chains.get(&transfer_reveal_nonce.state_chain_id).is_none() {
        return Err(SEError::Generic(String::from("State chain ID not in this batch.")));
    }

    verify_commitment(
        &transfer_reveal_nonce.hash,
        &transfer_reveal_nonce.state_chain_id,
        &transfer_reveal_nonce.nonce,
    )?;

    // If state chain completed + commitment revealed then punishment can be removed from state chain
    if *transfer_batch_data.state_chains.get(&transfer_reveal_nonce.state_chain_id).unwrap() {
        let mut state_chain: StateChain =
            db::get(&state.db, &claim.sub, &transfer_reveal_nonce.state_chain_id, &StateEntityStruct::StateChain)?
                .ok_or(SEError::DBError(NoDataForID, transfer_reveal_nonce.state_chain_id.clone()))?;

        state_chain.locked_until = SystemTime::now();
        db::insert(
            &state.db,
            &claim.sub,
            &transfer_reveal_nonce.state_chain_id,
            &StateEntityStruct::StateChain,
            &state_chain
        )?;

        // remove from transfer batch punished list
        transfer_batch_data.punished_state_chains.retain(|x| x != &transfer_reveal_nonce.state_chain_id);

        debug!("Commitment revealed for state chain ID {} in batch ID: {}",
            transfer_reveal_nonce.state_chain_id, transfer_reveal_nonce.batch_id);
    }

    db::insert(
        &state.db,
        &claim.sub,
        &transfer_reveal_nonce.batch_id,
        &StateEntityStruct::TransferBatchData,
        &transfer_batch_data
    )?;

    Ok(Json(()))
}
