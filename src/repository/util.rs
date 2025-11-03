use sqlx::{Error, Pool, Postgres, Transaction};

pub async fn start_transaction(pool: &Pool<Postgres>) -> Result<Transaction<Postgres>, Error> {
    let tx = pool.begin().await?;
    Ok(tx)
}