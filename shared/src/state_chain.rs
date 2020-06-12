//! State Chain
//!
//! State chain is the data structure used to track ownership of a UTXO co-owned by the State Entity.
//! An owner provides a key (we call proof key) which gets appended to the state chain once their
//! ownership is confirmed.
//! Then, to pass ownership over to a new proof key the current owner signs a StateChainSig struct
//! which includes the new owners proof key. This new proof key is then appended to the state chain
//! as before. Thus ownership can be verified by ensuring the newest proof key has been signed for by the
//! previous proof key.
//! To withdraw, and hence bring an end to the State Chain, the StateChainSig struct contains the
//! withdrawal address.

use super::Result;
use crate::error::SharedLibError;

use bitcoin::secp256k1::{Signature, SecretKey, Message, Secp256k1, PublicKey};
use bitcoin::hashes::{sha256d,Hash};
use bitcoin::Transaction;
use uuid::Uuid;

use monotree::tree::verify_proof;
use monotree::{Monotree, Proof};
use monotree::database::RocksDB;
use monotree::hasher::{Hasher,Blake2b};

use std::str::FromStr;
use std::convert::TryInto;

/// A list of States in which each State signs for the next State.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct StateChain {
    pub id: String,
    /// chain of transitory key history
    pub chain: Vec<State>,
    /// current back-up transaction
    pub tx_backup: Transaction,
    /// Amount
    pub amount: u64 // 0 means state chain is ended.
}

impl StateChain {
    pub fn new(data: String, tx_backup: Transaction, amount: u64) -> Self {
        StateChain {
            id: Uuid::new_v4().to_string(),
            chain: vec!( State {
                data,
                next_state: None
            }),
            tx_backup,
            amount
        }
    }

    pub fn get_tip(&self) -> Result<State> {
        Ok(self.chain.last()
            .ok_or(SharedLibError::Generic(String::from("StateChain empty")))?.clone())
    }

    pub fn add(&mut self, state_chain_sig: StateChainSig) -> Result<()> {
        let mut tip = self.get_tip()?;

        // verify previous state has signature and signs for new proof_key
        let prev_proof_key = tip.data.clone();
        state_chain_sig.verify(&prev_proof_key)?;

        // add sig to current tip
        tip.next_state = Some(state_chain_sig.clone());
        self.chain.pop();
        self.chain.push(tip);

        // add new tip to chain
        Ok(self.chain.push(State {
            data: state_chain_sig.data,
            next_state: None
        }))
    }
}


/// each State in the Chain of States
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct State {
    pub data: String,   // proof key or address
    pub next_state: Option<StateChainSig> // signature representing passing of ownership
}
/// Data necessary to create ownership transfer signatures
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct StateChainSig {
    pub purpose: String, // "TRANSFER" or "WITHDRAW"
    pub data: String, // proof key or address
    sig: String
}
impl StateChainSig {
    /// create message to be signed
    fn to_message(purpose: &String, data: &String) -> Result<Message> {
        let mut str = purpose.clone();
        str.push_str(&data);    // append data to msg
        let hash = sha256d::Hash::hash(&str.as_bytes());
        Ok(Message::from_slice(&hash)?)
    }

    /// generate signature for change of state chain ownership
    pub fn new(proof_key_priv: &SecretKey, purpose: &String, data: &String) -> Result<Self> {
        let secp = Secp256k1::new();
        let message = StateChainSig::to_message(purpose, data)?;
        let sig = secp.sign(&message, &proof_key_priv);
        Ok(StateChainSig {
            purpose: purpose.clone(),
            data: data.clone(),
            sig: sig.to_string()
        })
    }

    /// verify self's signature for transfer or withdraw
    pub fn verify(&self, pk: &String) -> Result<()> {
        let secp = Secp256k1::new();
        let message = StateChainSig::to_message(&self.purpose, &self.data)?;
        Ok(secp.verify(
            &message,
            &Signature::from_str(&self.sig).unwrap(),
            &PublicKey::from_str(&pk).unwrap()
        )?)
    }
}


/// insert new statechain entry into Sparse Merkle Tree and return proof
pub fn update_statechain_smt(sc_db_loc: &str, root: &Option<monotree::Hash>, funding_txid: &String, entry: &String) -> Result<Option<monotree::Hash>> {
    let key: &monotree::Hash = funding_txid[..32].as_bytes().try_into().unwrap();
    let entry: &monotree::Hash = entry[..32].as_bytes().try_into().unwrap();

    // update smt
    let mut tree = Monotree::<RocksDB, Blake2b>::new(sc_db_loc);
    let new_root = tree.insert(root.as_ref(), key, entry)?;

    Ok(new_root)
}

// Method can run as a seperate proof generation daemon. Must check root exists before calling.
pub fn gen_proof_smt(sc_db_loc: &str, root: &Option<monotree::Hash>, funding_txid: &String) -> Result<Option<Proof>> {
    let key: &monotree::Hash = funding_txid[..32].as_bytes().try_into().unwrap();
    let mut tree = Monotree::<RocksDB, Blake2b>::new(sc_db_loc);

    // generate inclusion proof
    let proof = tree.get_merkle_proof(root.as_ref(), key)?;
    Ok(proof)
}

pub fn verify_statechain_smt(root: &Option<monotree::Hash>, proof_key: &String, proof: &Option<Proof>) -> bool {
    let entry: &monotree::Hash = proof_key[..32].as_bytes().try_into().unwrap();
    let hasher = Blake2b::new();
    verify_proof(&hasher, root.as_ref(), &entry, proof.as_ref())
}


#[cfg(test)]
mod tests {

    use super::*;

    use bitcoin::secp256k1::{SecretKey, Secp256k1, PublicKey};

    pub static DB_LOC: &str = "./db";

    #[test]
    fn test_add_to_state_chain() {
        let secp = Secp256k1::new();
        let proof_key1_priv = SecretKey::from_slice(&[1;32]).unwrap();
        let proof_key1_pub = PublicKey::from_secret_key(&secp, &proof_key1_priv);

        let mut state_chain = StateChain::new(
            proof_key1_pub.to_string(),
            Transaction{version: 2,lock_time: 0,input: vec!(),output: vec!()},
            1000
        );

        assert_eq!(state_chain.chain.len(),1);

        // StateChainSig.verify called in function below
        let new_state_sig = StateChainSig::new(
            &proof_key1_priv,
            &String::from("TRANSFER"),
            &String::from("03b971d624567214a2e9a53995ee7d4858d6355eb4e3863d9ac540085c8b2d12b3"),
        ).unwrap();

        // add to state chain
        let _ = state_chain.add(new_state_sig.clone());
        assert_eq!(state_chain.chain.len(),2);

        // try add again (signature no longer valid for proof key "03b971d624567214a2e9a53995ee7d4858d6355eb4e3863d9ac540085c8b2d12b3")
        let fail = state_chain.add(new_state_sig);
        assert!(fail.is_err());
    }

    #[test]
    fn test_update_and_prove_sc_smt() {
        let funding_txid = String::from("c1562f7f15d6b8a51ea2e7035b9cdb8c6c0c41fecb62d459a3a6bf738ff0db0e");
        let proof_key = String::from("03b971d624567214a2e9a53995ee7d4858d6355eb4e3863d9ac540085c8b2d12b3");

        let root = None;

        let root = update_statechain_smt(DB_LOC, &root, &funding_txid, &proof_key).unwrap();

        let sc_smt_proof1 = gen_proof_smt(DB_LOC, &root, &funding_txid).unwrap();

        assert!(verify_statechain_smt(&root, &proof_key, &sc_smt_proof1));

        // update with new proof key and try again
        let proof_key = String::from("13b971d624567214a2e9a53995ee7d4858d6355eb4e3863d9ac540085c8b2d12b3");
        let root = update_statechain_smt(DB_LOC, &root, &funding_txid, &proof_key).unwrap();


        let sc_smt_proof2 = gen_proof_smt(DB_LOC, &root, &funding_txid).unwrap();
        assert!(verify_statechain_smt(&root, &proof_key, &sc_smt_proof2));
    }
}
