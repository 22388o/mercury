#![allow(unused_parens)]
#![recursion_limit = "128"]
#![feature(proc_macro_hygiene)]
#![feature(decl_macro)]
#[macro_use]
extern crate rocket;
extern crate config;
extern crate curv;
extern crate kms;
extern crate multi_party_ecdsa;
extern crate rocket_contrib;
extern crate rocksdb;
extern crate uuid;
extern crate zk_paillier;
#[macro_use]
extern crate failure;

extern crate error_chain;

#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate serde_json;

#[macro_use]
extern crate log;

#[cfg(test)]
extern crate floating_duration;

extern crate crypto;
extern crate jsonwebtoken as jwt;
extern crate rusoto_dynamodb;
extern crate serde_dynamodb;

extern crate hex;
extern crate shared_lib;
use shared_lib::mainstay;

#[macro_use]
extern crate serial_test;

pub mod auth;
pub mod routes;
pub mod server;
pub mod storage;
pub mod tests;
pub mod error;

type Result<T> = std::result::Result<T, error::SEError>;

pub struct Config {
    pub db: rocksdb::DB,
    pub electrum_server: String,
    pub network: String,
    pub testing_mode: bool,  // set for testing mode
    pub fee_address: String, // receive address for fee payments
    pub fee_deposit: u64, // satoshis
    pub fee_withdraw: u64, // satoshis
    pub block_time: u64,
    pub batch_lifetime: u64,
    pub punishment_duration: u64,
    pub mainstay_config: Option<mainstay::Config>
    
}

#[derive(Deserialize)]
pub struct AuthConfig {
    pub issuer: String,
    pub audience: String,
    pub region: String,
    pub pool_id: String,
}
