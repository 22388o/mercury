//! Transfer
//!
//! Transfer coins in state entity to new owner

// transfer() messages:
// 0. Receiver communicates address to Sender (B2 and C2)
// 1. Sender Initialises transfer protocol with State Entity
//      a. init, authorisation, provide receivers proofkey C2
//      b. State Entity generate x1 and sends to Sender
//      c. Sender and State Entity Co-sign new back up transaction sending to Receivers
//          backup address Addr(B2)
// 2. Receiver performs transfer with State Entity
//      a. Verify state chain is updated
//      b. calculate t1=01x1
//      c. calucaulte t2 = t1*o2_inv
//      d. Send t2, O2 to state entity
//      e. Verify o2*S2 = P

use super::super::Result;
use shared_lib::structs::{StateChainDataAPI, StateEntityAddress, TransferMsg1, TransferMsg2, TransferMsg3, TransferMsg4, TransferMsg5, Protocol};
use shared_lib::state_chain::StateChainSig;

use crate::error::CError;
use crate::wallet::wallet::Wallet;
use crate::wallet::key_paths::funding_txid_to_int;
use crate::state_entity::util::{cosign_tx_input,verify_statechain_smt};
use crate::state_entity::api::{get_smt_proof, get_smt_root, get_statechain};
use super::super::utilities::requests;

use bitcoin::{Address,PublicKey};
use curv::elliptic::curves::traits::{ECPoint, ECScalar};
use curv::{FE, GE};
use std::str::FromStr;


/// Transfer coins to new Owner from this wallet
pub fn transfer_sender(
    wallet: &mut Wallet,
    shared_key_id: &String,
    receiver_addr: StateEntityAddress,
) -> Result<TransferMsg3> {
    // Get required shared key data
    let state_chain_id;
    let mut prepare_sign_msg;
    {
        let shared_key = wallet.get_shared_key(shared_key_id)?;
        state_chain_id = shared_key.state_chain_id.clone()
            .ok_or(CError::Generic(String::from("No state chain for this shared key id")))?;
        prepare_sign_msg = shared_key.tx_backup_psm.clone()
            .ok_or(CError::Generic(String::from("No back up transaction data found for this shared key id")))?;
    }

    // first sign state chain
    let state_chain_data: StateChainDataAPI = get_statechain(&wallet.client_shim, &state_chain_id)?;
    let state_chain = state_chain_data.chain;
    // get proof key for signing
    let proof_key_derivation = wallet.se_proof_keys.get_key_derivation(&PublicKey::from_str(&state_chain.last().unwrap().data).unwrap());
    let state_chain_sig = StateChainSig::new(
        &proof_key_derivation.unwrap().private_key.key,
        &String::from("TRANSFER"),
        &receiver_addr.proof_key.clone().to_string()
    )?;

    // init transfer: perform auth and send new statechain
    let transfer_msg2: TransferMsg2 = requests::postb(&wallet.client_shim,&format!("/transfer/sender"),
        &TransferMsg1 {
            shared_key_id: shared_key_id.to_string(),
            state_chain_sig: state_chain_sig.clone()
        })?;

    // update prepare_sign_msg with new owners address, proof key
    prepare_sign_msg.protocol = Protocol::Transfer;
    prepare_sign_msg.tx.output.get_mut(0).unwrap().script_pubkey = Address::from_str(&receiver_addr.tx_backup_addr)?.script_pubkey();
    prepare_sign_msg.proof_key = Some(receiver_addr.proof_key.clone().to_string());

    // sign new back up tx
    let new_backup_witness = cosign_tx_input(wallet, &shared_key_id, &prepare_sign_msg)?;
    // update back up tx with new witness
    prepare_sign_msg.tx.input[0].witness = new_backup_witness;

    // get o1 priv key
    let shared_key = wallet.get_shared_key(&shared_key_id)?;
    let o1 = shared_key.share.private.get_private_key();

    // t1 = o1x1
    let t1 = o1 * transfer_msg2.x1;

    let transfer_msg3 = TransferMsg3 {
        shared_key_id: shared_key_id.to_string(),
        t1, // should be encrypted
        state_chain_sig,
        state_chain_id: state_chain_id.to_string(),
        tx_backup_psm: prepare_sign_msg.to_owned(),
        rec_addr: receiver_addr
    };

    // Mark funds as spent in wallet
    {
        let mut shared_key = wallet.get_shared_key_mut(shared_key_id)?;
        shared_key.unspent = false;
    }

    Ok(transfer_msg3)
}

/// Receiver side of Transfer protocol.
pub fn transfer_receiver(
    wallet: &mut Wallet,
    transfer_msg3: &TransferMsg3,
) -> Result<String> {
    // get statechain data (will Err if statechain not yet finalized)
    let state_chain_data: StateChainDataAPI = get_statechain(&wallet.client_shim, &transfer_msg3.state_chain_id)?;

    // verify state chain represents this address as new owner
    let prev_owner_proof_key = state_chain_data.chain.last().unwrap().data.clone();
    match transfer_msg3.state_chain_sig.verify(&prev_owner_proof_key) {
        Ok(_) => debug!("State chain signature is valid."),
        Err(_) => return Err(CError::Generic(String::from("State Chain verification failed.")))
    }


    // check try_o2() comments and docs for justification of below code
    let mut done = false;
    let mut transfer_msg5 = TransferMsg5::default();
    let mut o2 = FE::zero();
    let mut num_tries = 0;
    while !done {
        match try_o2(wallet, &state_chain_data, transfer_msg3, &num_tries) {
            Ok(success_resp) => {
                o2 = success_resp.0.clone();
                transfer_msg5 = success_resp.1.clone();
                done = true;
            },
            Err(e) => {
                if !e.to_string().contains(&String::from("Error: Invalid o2, try again.")) {
                    return Err(e);
                }
                num_tries = num_tries + 1;
                debug!("try o2 failure. Trying again...");
            }
        }
    }

    // Make shared key with new private share
    let shared_id = &transfer_msg5.new_shared_key_id;
    wallet.gen_shared_key_fixed_secret_key(shared_id, &o2.get_element(), &state_chain_data.amount)?;

    // Check shared key master public key == private share * SE public share
    if (transfer_msg5.s2_pub*o2).get_element()
        != wallet.get_shared_key(&shared_id)?.share.public.q.get_element() {
            return Err(CError::StateEntityError(String::from("Transfer failed. Incorrect master public key generated.")))
    }

    // TODO when node is integrated: Should also check that funding tx output address is address derived from shared key.

    let rec_proof_key = transfer_msg3.rec_addr.proof_key.clone();

    // verify proof key inclusion in SE sparse merkle tree
    let root = get_smt_root(wallet)?;
    let proof = get_smt_proof(wallet, &root, &state_chain_data.utxo.txid.to_string())?;
    assert!(verify_statechain_smt(
        &root.value,
        &rec_proof_key,
        &proof
    ));

    // Add state chain id, proof key and SMT inclusion proofs to local SharedKey data
    {
        let shared_key = wallet.get_shared_key_mut(&shared_id)?;
        shared_key.state_chain_id = Some(transfer_msg3.state_chain_id.to_string());
        shared_key.tx_backup_psm = Some(transfer_msg3.tx_backup_psm.clone());
        shared_key.add_proof_data(&rec_proof_key, &root, &proof);
    }

    Ok(transfer_msg5.new_shared_key_id)
}

// Constraint on s2 size means that some (most) o2 values are not valid for the lindell_2017 protocol.
// We must generate random o2, test if the resulting s2 is valid and try again if not.
/// Carry out transfer_receiver() protocol with a randomly generated o2 value.
pub fn try_o2(wallet: &mut Wallet, state_chain_data: &StateChainDataAPI, transfer_msg3: &TransferMsg3, num_tries: &u32) -> Result<(FE,TransferMsg5)>{
    // generate o2 private key and corresponding 02 public key
    let mut encoded_txid = num_tries.to_string();
    encoded_txid.push_str(&state_chain_data.utxo.txid.to_string());
    let key_share_pub = wallet.se_key_shares.get_new_key_encoded_id(
        funding_txid_to_int(&encoded_txid)?
    )?;
    let key_share_priv = wallet.se_key_shares.get_key_derivation(&key_share_pub).unwrap().private_key.key;
    let mut o2: FE = ECScalar::zero();
    o2.set_element(key_share_priv);

    let g: GE = ECPoint::generator();
    let o2_pub: GE = g * o2;

    // decrypt t1

    // t2 = t1*o2_inv = o1*x1*o2_inv
    let t2 = transfer_msg3.t1 * (o2.invert());

    // encrypt t2 with SE key and sign with Receiver proof key (se_addr.proof_key)

    let transfer_msg5: TransferMsg5 = requests::postb(&wallet.client_shim,&format!("/transfer/receiver"),
        &TransferMsg4 {
            shared_key_id: transfer_msg3.shared_key_id.clone(),
            t2, // should be encrypted
            state_chain_sig: transfer_msg3.state_chain_sig.clone(),
            o2_pub
        })?;
    Ok((o2,transfer_msg5))

}

#[cfg(test)]
mod tests {

    // use curv::elliptic::curves::traits::{ECPoint, ECScalar};
    // use curv::{FE, GE};

    // #[test]
    // fn math() {
    //     let g: GE = ECPoint::generator();
    //
    //     //owner1 share
    //     let o1_s: FE = ECScalar::new_random();
    //     let o1_p: GE = g * o1_s;
    //
    //     // SE share
    //     let s1_s: FE = ECScalar::new_random();
    //     let s1_p: GE = g * s1_s;
    //
    //     // deposit P
    //     let p_p = s1_p*o1_s;
    //     println!("P1: {:?}",p_p);
    //     let p_p = o1_p*s1_s;
    //     println!("P1: {:?}",p_p);
    //
    //
    //     // transfer
    //     // SE new random key x1
    //     let x1_s: FE = ECScalar::new_random();
    //
    //     // owner2 share
    //     let o2_s: FE = ECScalar::new_random();
    //     let o2_p: GE = g * o2_s;
    //
    //     // t1 = o1*x1*o2_inv
    //     let t1 = o1_s*x1_s*(o2_s.invert());
    //
    //     // t2 = t1*x1_inv*s1
    //     let s2_s = t1*(x1_s.invert())*s1_s;
    //
    //     println!("P2: {:?}",o2_p*s2_s);
    // }
}
