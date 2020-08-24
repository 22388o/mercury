pub use super::Result;
extern crate shared_lib;
use crate::error::{DBErrorType, SEError};
use crate::server::StateChainEntity;
use crate::storage::Storage;
use crate::Database;

use std::{thread, time};

use bitcoincore_rpc::{Auth, Client, RpcApi};
use bitcoin::{Transaction,
    hashes::sha256d};
use bitcoin::consensus::encode;


pub fn watch_node(rpc_path: String) {

    let interval = time::Duration::from_millis(100);

    let rpc_path_parts: Vec<&str> = rpc_path.split('@').collect();
    if rpc_path_parts.len() != 2 {
        panic!("Invalid bitcoind RPC path")
    };

    let rpc_cred: Vec<&str> = rpc_path_parts[0].split(':').collect();
    if rpc_cred.len() != 2 {
        panic!("Invalid bitcoind RPC credentials")
    };

    let rpc = Client::new(rpc_path_parts[1].to_string(),
                          Auth::UserPass(rpc_cred[0].to_string(),
                                         rpc_cred[1].to_string())).unwrap();

    // main watch loop
    loop {
        // get current block height
        let bestblockcount = rpc.get_block_count();
        let blocks = bestblockcount.unwrap();

        println!("{} blocks",blocks);

        // find valid backup transactions
        // iterate through backup transaction db
//        let mut iter = config.db.iterator(IteratorMode::Start); // Always iterates forward






//        for (key, value) in iter {
            //if backup tx has valid locktime, then broadcast 
//            if value.locktime.to_u64() <= blocks {
//                let tx = value.tx;
//                let tx_ser = &encode::serialize_hex(tx);
//                let senttx = rpc.send_raw_transaction(tx_ser);
                //if already confirmed - remove tx from database
//                if let Err(Error::JsonRpc(jsonrpc::error::Error::Rpc(ref rpcerr))) = senttx {
//                    if rpcerr.code == -28 
//                    {
//                        // remove transaction from backup DB
//                    }
//                }
//            }
//        }
        thread::sleep(interval);
    }
}