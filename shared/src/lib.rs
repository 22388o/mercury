//
extern crate hex;
extern crate bitcoin;
extern crate kms;
extern crate rocket;
extern crate rocket_contrib;
extern crate rocksdb;
extern crate uuid;

#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate serde_json;

extern crate itertools;

extern crate reqwest;
extern crate base64;
pub mod util;
pub mod error;
pub mod structs;
pub mod state_chain;
pub mod mocks;
pub mod mainstay;

type Result<T> = std::result::Result<T, error::SharedLibError>;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Root {
    pub id: u32,
    pub value: Option<[u8;32]>
}
