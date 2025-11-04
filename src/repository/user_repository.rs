use sqlx::{query_as, Error, Pool, Postgres, Transaction};
use uuid::Uuid;
use crate::user_relationship::model::{UserWithRelationship};

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

    pub async fn find_user_by_id_with_relationship_type(&self, client_id: &Uuid, searched_user_id: &Uuid) -> Result<Option<UserWithRelationship>, Error> {
        let user = query_as::<_, UserWithRelationship>(
            r#"SELECT
                r_user.id,
                r_user.display_name,
                r_user.profile_picture,
                r_user.street_credits,
                r_user.description,
                r_user.friends_count,
                user_relationship.user_a_id,
                user_relationship.user_b_id,
                user_relationship.state,
                user_relationship.relationship_change_timestamp
                FROM app_user r_user
                LEFT JOIN user_relationship ON
                    (user_relationship.user_a_id = r_user.id AND user_relationship.user_b_id = $2) OR
                    (user_relationship.user_b_id = r_user.id AND user_relationship.user_a_id = $2)
                WHERE r_user.id = $1 AND r_user.id <> $2
            "#
        )
            .bind(searched_user_id)
            .bind(client_id)
            .fetch_optional(&self.pool).await?;
        Ok(user)
    }

}