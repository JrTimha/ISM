use sqlx::{Error, Pool, Postgres, Transaction};


#[derive(Clone)]
pub struct UserRepository {
    pool: Pool<Postgres>,
}

impl UserRepository {

    pub fn new(pool: Pool<Postgres>) -> Self {
        UserRepository { pool }
    }

    pub async fn start_transaction(&self) -> Result<Transaction<'_, Postgres>, Error> {
        let tx = self.pool.begin().await?;
        Ok(tx)
    }

}