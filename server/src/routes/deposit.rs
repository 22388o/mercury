//! StateEntity Deposit
//!
//! StateEntity Deposit protocol.

use super::super::{{Result,Config},
    auth::jwt::Claims,
    storage::db};
extern crate shared_lib;
use shared_lib::{
    util::FEE,
    structs::*,
    state_chain::*,
    Root,
    mocks::mock_electrum::MockElectrum};
use crate::error::SEError;
use crate::storage::db_postgres::{Table, Column, db_insert, db_deser, db_ser, db_update, db_get_2};
use crate::DataBase;
use bitcoin::Transaction;

use electrumx_client::{
    interface::Electrumx,
    electrumx_client::ElectrumxClient};

use rocket_contrib::json::Json;
use rocket::State;
use uuid::Uuid;
use db::{DB_SC_LOC, update_root, get_current_root};
use std::{thread,
    time::Duration};

/// Initiliase deposit protocol:
///     - Generate and return shared wallet ID
///     - Can do auth or other DoS mitigation here
#[post("/deposit/init", format = "json", data = "<deposit_msg1>")]
pub fn deposit_init(
    conn: DataBase,
    deposit_msg1: Json<DepositMsg1>,
) -> Result<Json<Uuid>> {
    // Generate shared wallet ID (user ID)
    let user_id = Uuid::new_v4();

    // if Verification/PoW/authoriation failed {
    //      warn!("Failed authorisation.")
    //      Err(SEError::AuthError)
    //  }

    // Create DB entry for newly generated ID signalling that user has passed some
    // verification. For now use ID as 'password' to interact with state entity
    db_insert(&conn, &user_id, Table::UserSession)?;
    db_update(&conn, &user_id, Table::UserSession,
        vec!(Column::Authentication,Column::ProofKey),
        vec!(&deposit_msg1.auth.clone(),&deposit_msg1.proof_key.to_owned()))?;

    info!("DEPOSIT: Protocol initiated. User ID generated: {}",user_id);
    debug!("DEPOSIT: User ID: {} corresponding Proof key: {}", user_id, deposit_msg1.proof_key.to_owned());

    Ok(Json(user_id))
}

/// Query an Electrum Server for a transaction's confirmation status.
/// Return Ok() if confirmed or Error if not after some waiting period.
pub fn verify_tx_confirmed(txid: &String, state: &State<Config>) -> Result<()> {
    let mut electrum: Box<dyn Electrumx> = if state.testing_mode {
        Box::new(MockElectrum::new())
    } else {
        Box::new(ElectrumxClient::new(state.electrum_server.clone()).unwrap())
    };

    info!("DEPOSIT: Waiting for funding transaction confirmation. Txid: {}",txid);

    let mut is_broadcast = 0;   // num blocks waited for tx to be broadcast
    let mut is_mined = 0;       // num blocks waited for tx to be mined
    while is_broadcast < 3 {    // Check for tx broadcast. If not after 3*(block time) then return error.
        match electrum.get_transaction_conf_status(txid.clone(), false) {
            Ok(res) => {
                // Check for tx confs. If none after 10*(block time) then return error.
                if res.confirmations.is_none() {
                    is_mined += 1;
                    if is_mined > 9 {
                        warn!("Funding transaction not mined after 10 blocks. Deposit failed. Txid: {}", txid);
                        return Err(SEError::Generic(String::from("Funding transaction failure to be mined - consider increasing the fee. Deposit failed.")));
                    }
                    thread::sleep(Duration::from_millis(state.block_time));
                } else { // If confs increase then wait 6*(block time) and return Ok()
                    info!("Funding transaction mined. Waiting for 6 blocks confirmation. Txid: {}",txid);
                    thread::sleep(Duration::from_millis(6*state.block_time));
                    return Ok(())
                }
            },
            Err(_) => {
                is_broadcast += 1;
                thread::sleep(Duration::from_millis(state.block_time));
            }
        }
    }
    return Err(SEError::Generic(String::from("Funding Transaction not found in blockchain. Deposit failed.")));
}

/// Final step in deposit protocol:
///     - Wait for confirmation of funding tx in blockchain
///     - Create StateChain DB object
///     - Update sparse merkle tree with new StateChain entry
#[post("/deposit/confirm", format = "json", data = "<deposit_msg2>")]
pub fn deposit_confirm(
    state: State<Config>,
    _claim: Claims,
    conn: DataBase,
    deposit_msg2: Json<DepositMsg2>,
) -> Result<Json<Uuid>> {
    // let shared_key_id = deposit_msg2.shared_key_id.clone();
    let user_id = deposit_msg2.shared_key_id;

    // Get back up tx and proof key
    let (tx_backup_str, proof_key) = db_get_2::<String,String>(&conn, &user_id, Table::UserSession,
        vec!(Column::TxBackup, Column::ProofKey))?;
    let tx_backup: Transaction = db_deser(tx_backup_str)?;

    // Ensure backup tx exists is signed
    if tx_backup.input[0].witness.len() == 0 {
        return Err(SEError::Generic(String::from("Signed Back up transaction not found.")));
    }

    // Wait for funding tx existence in blockchain and confs
    verify_tx_confirmed(&tx_backup.input[0].previous_output.txid.to_string(), &state)?;

    // Create state chain DB object
    let state_chain_id = Uuid::new_v4();
    let amount = (tx_backup.output.last().unwrap().value  + FEE) as i64;
    let state_chain = StateChain::new(
        proof_key.clone(),
    );

    // Insert into StateChain table
    db_insert(&conn, &state_chain_id, Table::StateChain)?;
    db_update(&conn, &state_chain_id, Table::StateChain,
        vec!(
            Column::Chain,
            Column::Amount,
            Column::LockedUntil,
            Column::OwnerId),
        vec!(
            &db_ser(state_chain)?,
            &amount,
            &get_time_now(),
            &user_id.to_owned()))?;

    // Insert into BackupTx table
    db_insert(&conn, &state_chain_id, Table::BackupTxs)?;
    db_update(&conn, &state_chain_id, Table::BackupTxs,vec!(Column::TxBackup),vec!(&db_ser(tx_backup.clone())?))?;

    info!("DEPOSIT: State Chain created. ID: {} For user ID: {}", state_chain_id, user_id);


    // Update sparse merkle tree with new StateChain entry
    let root = get_current_root::<Root>(&state.db)?.map(|r| r.hash());

    let new_root_hash = &update_statechain_smt(
        DB_SC_LOC,
        &root,
        &tx_backup.input.get(0).unwrap().previous_output.txid.to_string(),
        &proof_key
    )?;

    let new_root = Root::from_hash(&new_root_hash.unwrap());

    update_root(&state.db, &state.mainstay_config, &new_root)?;

    info!("DEPOSIT: Included in sparse merkle tree. State Chain ID: {}", state_chain_id);
    debug!("DEPOSIT: State Chain ID: {}. New root: {:?}. Previous root: {:?}.", state_chain_id, new_root, root);

    // Update UserSession with StateChain's ID
    db_update(&conn, &user_id, Table::UserSession,vec!(Column::StateChainId),vec!(&state_chain_id))?;

    Ok(Json(state_chain_id))
}
