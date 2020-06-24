extern crate server_lib;
extern crate client_lib;
extern crate shared_lib;
extern crate bitcoin;

use client_lib::*;
use client_lib::wallet::wallet::Wallet;

use server_lib::server;
use shared_lib::{
    mocks::mock_electrum::MockElectrum,
    structs::PrepareSignTxMsg};

use bitcoin::{Transaction, PublicKey};

use std::{thread, time};

pub fn spawn_server() {
    // Rocket server is blocking, so we spawn a new thread.
    thread::spawn(move || {
        server::get_server().launch();
    });

    let five_seconds = time::Duration::from_millis(5000);
    thread::sleep(five_seconds);
}

pub fn gen_wallet() -> Wallet {
    let mut wallet = Wallet::new(
        &[0xcd; 32],
        &"regtest".to_string(),
        ClientShim::new("http://localhost:8000".to_string(), None),
        Box::new(MockElectrum::new())
    );

    // generate some addresses
    let _ = wallet.keys.get_new_address();
    let _ = wallet.keys.get_new_address();

    wallet
}

// Returns shared_key_id, state_chain_id, funding txid,
/// signed backup tx, back up transacion data and proof_key
pub fn run_deposit(wallet: &mut Wallet, amount: &u64) -> (String, String, String, Transaction, PrepareSignTxMsg, PublicKey)  {
    let resp = state_entity::deposit::deposit(
        wallet,
        amount
    ).unwrap();

    return resp
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate server_lib;
    extern crate client_lib;
    extern crate shared_lib;
    extern crate bitcoin;

    use shared_lib::mocks::mock_electrum::MockElectrum;

    use curv::elliptic::curves::traits::ECScalar;
    use curv::FE;


    #[test]
    fn test_gen_shared_key() {
        spawn_server();
        let mut wallet = gen_wallet();
        let proof_key = wallet.se_proof_keys.get_new_key().unwrap();
        let init_res = client_lib::state_entity::deposit::session_init(&mut wallet, &proof_key.to_string());
        assert!(init_res.is_ok());
        let key_res = wallet.gen_shared_key(&init_res.unwrap(), &1000);
        assert!(key_res.is_ok());
    }

    #[test]
    fn test_failed_auth() {
        spawn_server();
        let client_shim = ClientShim::new("http://localhost:8000".to_string(), None);
        let secret_key: FE = ECScalar::new_random();
        let err = ecdsa::get_master_key(&"Invalid id".to_string(), &client_shim, &secret_key, &1000, false);
        assert!(err.is_err());
    }

    // #[test]
    // fn test_schnorr() {
    //     spawn_server();
    //
    //     let client_shim = ClientShim::new("http://localhost:8000".to_string(), None);
    //
    //     let share: schnorr::Share = schnorr::generate_key(&client_shim).unwrap();
    //
    //     let msg: BigInt = BigInt::from(1234);  // arbitrary message
    //     let signature = schnorr::sign(&client_shim, msg, &share)
    //         .expect("Schnorr signature failed");
    //
    //     println!(
    //         "signature = (e: {:?}, s: {:?})",
    //         signature.e,
    //         signature.s
    //     );
    // }

    #[test]
    fn test_deposit() {
        spawn_server();
        let mut wallet = gen_wallet();

        let deposit = run_deposit(&mut wallet, &10000);

        let funding_txid = deposit.2;
        let tx_backup_psm = deposit.4;
        let proof_key = deposit.5;

        // Get SMT inclusion proof and verify
        let root = state_entity::api::get_smt_root(&wallet.client_shim).unwrap();
        let proof = state_entity::api::get_smt_proof(&wallet.client_shim, &root, &funding_txid).unwrap();

        //ensure wallet's shared key is updated with proof info
        let shared_key = wallet.get_shared_key(&deposit.0).unwrap();
        assert_eq!(shared_key.smt_proof.clone().unwrap().root, root);
        assert_eq!(shared_key.smt_proof.clone().unwrap().proof, proof);
        assert_eq!(shared_key.proof_key.clone().unwrap(), proof_key.to_string());

        println!("Shared wallet id: {:?} ",deposit.0);
        println!("Funding txid: {:?} ",funding_txid);
        println!("Back up transaction data: {:?} ",tx_backup_psm);
    }

    #[test]
    fn test_get_statechain() {
        spawn_server();
        let mut wallet = gen_wallet();

        let err = state_entity::api::get_statechain(&wallet.client_shim, &String::from("id"));
        assert!(err.is_err());

        let deposit = run_deposit(&mut wallet, &10000);

        let state_chain = state_entity::api::get_statechain(&wallet.client_shim, &String::from(deposit.1.clone())).unwrap();
        assert_eq!(state_chain.chain.last().unwrap().data, deposit.5.to_string());
    }

    #[test]
    fn test_transfer() {
        spawn_server();
        let mut wallet_sender = gen_wallet();

        let deposit_resp = run_deposit(&mut wallet_sender, &10000);

        // transfer
        let mut wallet_receiver = gen_wallet();
        let funding_txid = deposit_resp.2;
        let receiver_addr = wallet_receiver.get_new_state_entity_address(&funding_txid).unwrap();

        let tranfer_sender_resp =
            state_entity::transfer::transfer_sender(
                &mut wallet_sender,
                &deposit_resp.0,    // shared wallet id
                receiver_addr.clone(),
        ).unwrap();

        let new_shared_key_id  =
            state_entity::transfer::transfer_receiver(
                &mut wallet_receiver,
                &tranfer_sender_resp,
                &None
            ).unwrap().new_shared_key_id;

        // check shared keys have the same master public key
        assert_eq!(
            wallet_sender.get_shared_key(&deposit_resp.0).unwrap().share.public.q,
            wallet_receiver.get_shared_key(&new_shared_key_id).unwrap().share.public.q
        );

        // check state chain is updated
        let state_chain = state_entity::api::get_statechain(&wallet_sender.client_shim, &deposit_resp.1).unwrap();
        assert_eq!(state_chain.chain.len(),2);
        assert_eq!(state_chain.chain.last().unwrap().data.to_string(), receiver_addr.proof_key.to_string());

        // Get SMT inclusion proof and verify
        let root = state_entity::api::get_smt_root(&wallet_receiver.client_shim).unwrap();
        let proof = state_entity::api::get_smt_proof(&wallet_receiver.client_shim, &root, &funding_txid).unwrap();
        //ensure wallet's shared key is updated with proof info
        let shared_key = wallet_receiver.get_shared_key(&new_shared_key_id).unwrap();
        assert_eq!(shared_key.smt_proof.clone().unwrap().root, root);
        assert_eq!(shared_key.smt_proof.clone().unwrap().proof, proof);
        assert_eq!(shared_key.proof_key.clone().unwrap(),receiver_addr.proof_key);
    }



    #[test]
    fn test_withdraw() {
        spawn_server();
        let mut wallet = gen_wallet();

        let deposit_resp = run_deposit(&mut wallet, &10000);

        // check withdraw method completes without Err
        state_entity::withdraw::withdraw(&mut wallet, &deposit_resp.0)
            .unwrap();

        // check state chain is updated
        let state_chain = state_entity::api::get_statechain(&wallet.client_shim, &deposit_resp.1).unwrap();
        assert_eq!(state_chain.chain.len(),2);

        // check chain data is address
        assert!(state_chain.chain.last().unwrap().data.contains(&String::from("bcrt")));
        // check purpose of state chain signature
        assert_eq!(state_chain.chain.get(0).unwrap().next_state.clone().unwrap().purpose, String::from("WITHDRAW"));

        // Try again after funds already withdrawn
        let err = state_entity::withdraw::withdraw(&mut wallet, &deposit_resp.0);
        assert!(err.is_err());
    }

    #[test]
    fn test_wallet_load_with_shared_key() {
        spawn_server();

        let mut wallet = gen_wallet();
        run_deposit(&mut wallet, &10000);

        let wallet_json = wallet.to_json();
        let wallet_rebuilt = wallet::wallet::Wallet::from_json(wallet_json, ClientShim::new("http://localhost:8000".to_string(), None), Box::new(MockElectrum::new())).unwrap();

        let shared_key = wallet.shared_keys.get(0).unwrap();
        let shared_key_rebuilt = wallet_rebuilt.shared_keys.get(0).unwrap();

        assert_eq!(shared_key.id,shared_key_rebuilt.id);
        assert_eq!(shared_key.share.public, shared_key_rebuilt.share.public);
        assert_eq!(shared_key.proof_key, shared_key_rebuilt.proof_key);
        assert_eq!(shared_key.smt_proof.clone().unwrap().root, shared_key_rebuilt.smt_proof.clone().unwrap().root);
        assert_eq!(shared_key.smt_proof.clone().unwrap().proof, shared_key_rebuilt.smt_proof.clone().unwrap().proof);
    }



}
