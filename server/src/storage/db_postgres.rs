//! DB
//!
//! Postgres DB access and update tools.
//! Use db_get, db_update for rust types convertable to postgres types (String, int, Uuid, bool).
//! Use db_get_serialized, db_update_serialized for custom types.


use super::super::Result;

use rocket_contrib::databases::postgres::Connection;
use crate::error::{DBErrorType::{UpdateFailed,NoDataForID}, SEError};
use uuid::Uuid;
use shared_lib::state_chain::StateChain;
use std::time::SystemTime;

#[derive(Debug)]
pub enum Table {
    Testing,
    Ecdsa,
    UserSession,
    StateChain,
}
impl Table {
    fn to_string(&self) -> String {
        format!("{:?}", self)
    }
}

#[derive(Debug, Deserialize)]
pub enum Column {
    Data,
    Complete,

    // UserSession
    Id,
    Authentication,
    ProofKey,
    StateChainId,
    TxBackup,
    TxWithdraw,
    SigHash,
    S2,
    WithdrawScSig,

    // StateChain
    // Id,
    Chain,
    Amount,
    LockedUntil,
    OwnerId,

    KeyGenFirstMsg,
    CommWitness,
    EcKeyPair,
    PaillierKeyPair,
    Party1Private,
    Party2Public,

    PDLProver,
    PDLDecommit,
    Alpha,
    Party2PDLFirstMsg,

    Party1MasterKey,

    EphEcKeyPair,
    EphKeyGenFirstMsg,
    POS
}
impl Column {
    pub fn to_string(&self) -> String {
        format!("{:?}", self)
    }
}



// Create new item in table
pub fn db_insert(conn: &Connection, id: &Uuid, table: Table) -> Result<u64> {
    let statement = conn.prepare(&format!("INSERT INTO {} (id) VALUES ($1)",table.to_string()))?;

    Ok(statement.execute(&[id])?)
}

// Update item in table with PostgreSql data types (String, int, Uuid, bool)
pub fn db_update<T>(conn: &Connection, id: &Uuid, data: T, table: Table, column: Column) -> Result<()>
where
    T: rocket_contrib::databases::postgres::types::ToSql
{
    let statement = conn.prepare(&format!("UPDATE {} SET {} = $1 WHERE id = $2",table.to_string(),column.to_string()))?;
    if statement.execute(&[&data, &id])? == 0 {
        return Err(SEError::DBError(UpdateFailed, id.to_string()));
    }

    Ok(())
}

// Get item from table with PostgreSql data types (String, int, Uuid, bool)
// Err if ID not found. Return None if data item empty.
pub fn db_get<T>(conn: &Connection, id: &Uuid, table: Table, column: Column) -> Result<Option<T>>
where
    T: rocket_contrib::databases::postgres::types::FromSql
{
    let statement = conn.prepare(&format!("SELECT {} FROM {} WHERE id = $1",column.to_string(),table.to_string()))?;
    let rows = statement.query(&[&id])?;

    if rows.is_empty() {
        return Err(SEError::DBError(NoDataForID, id.to_string().clone()))
    };
    let row = rows.get(0);

    match row.get_opt::<usize, T>(0) {
        None => return Err(SEError::DBError(NoDataForID, id.to_string().clone())),
        Some(data) => {
            match data {
                Ok(v) => Ok(Some(v)),
                Err(_) => Ok(None)
            }
        }
    }
}

// Update item in table whose type is serialized to String
pub fn db_update_serialized<T>(conn: &Connection, id: &Uuid, data: T, table: Table, column: Column) -> Result<()>
where
    T: serde::ser::Serialize
{
    let item_string = serde_json::to_string(&data).unwrap();
    db_update(conn, id, item_string, table, column)
}

// Get item in table whose type is serialized to String
pub fn db_get_serialized<T>(conn: &Connection, id: &Uuid, table: Table, column: Column) -> Result<Option<T>>
where
    T: serde::de::DeserializeOwned,
{
    match db_get::<String>(conn, id, table, column)? {
        Some(data) => return Ok(Some(serde_json::from_str(&data).unwrap())),
        None => Ok(None)
    }
}

// Get entire row from statechain table.
// Err if ID not found. Return None if data item empty.
pub fn db_get_statechain(conn: &Connection, id: &Uuid) -> Result<StateChain> {
    let statement = conn.prepare("SELECT * FROM statechain WHERE id = $1")?;
    let rows = statement.query(&[&id])?;

    if rows.is_empty() {
        return Err(SEError::DBError(NoDataForID, id.to_string().clone()))
    };
    let row = rows.get(0);

    let id = row.get_opt::<usize,Uuid>(0).unwrap()?;
    let chain = serde_json::from_str(&row.get_opt::<usize,String>(1).unwrap()?).unwrap();
    let amount = row.get_opt::<usize,i64>(2).unwrap()?;
    // let locked_until = row.get_opt::<usize,String>(3).unwrap()?;
    let locked_until = SystemTime::now();
    let owner_id = row.get_opt::<usize,Uuid>(4).unwrap()?;

    Ok(StateChain {
        id,
        chain,
        amount,
        locked_until,
        owner_id
    })
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::{env, str::FromStr};

    #[test]
    fn test_db_postgres() {
        use postgres::{Connection, TlsMode};

        let rocket_url = env::var("ROCKET_DATABASES").unwrap();
        let url = &rocket_url[16..68];

        let conn = Connection::connect(url, TlsMode::None).unwrap();
        let user_id = Uuid::from_str(&"73c70459-3c8d-4628-891d-55276dc107fe").unwrap();
        let res = db_get_statechain(&conn, &user_id);
        println!("res: {:?}",res);

        let user_id = Uuid::from_str(&"0af4a3dc-a10b-47c8-b10c-01d4bdf6d5e0").unwrap();
        let res = db_get_statechain(&conn, &user_id);
        println!("res: {:?}",res);

        let user_id = Uuid::from_str(&"1af4a3dc-a10b-47c8-b10c-01d4bdf6d5e0").unwrap();
        let res = db_get_statechain(&conn, &user_id);
        println!("res: {:?}",res);

    }
}
