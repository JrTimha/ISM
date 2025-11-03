use sqlx::{Pool, Postgres};


#[derive(Clone)]
pub struct UserRepository {
    pool: Pool<Postgres>,
}

impl UserRepository {

    pub fn new(pool: Pool<Postgres>) -> Self {
        UserRepository { pool }
    }

}