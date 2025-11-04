use sqlx::{query_as, Error, PgConnection, Pool, Postgres, Transaction};
use uuid::Uuid;
use crate::user_relationship::model::{RelationshipState, User, UserPaginationCursor, UserRelationship, UserWithRelationship};

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

    pub async fn find_user_by_id(&self, user_id: &Uuid) -> Result<Option<User>, Error> {
        let user = query_as!(
                User,
                r#"SELECT
                    r_user.id,
                    r_user.display_name,
                    r_user.profile_picture,
                    r_user.street_credits,
                    r_user.description,
                    r_user.friends_count
                    FROM app_user r_user
                    WHERE r_user.id = $1
                "#, user_id
            ).fetch_optional(&self.pool).await?;
        Ok(user)
    }

    pub async fn find_user_by_name_with_relationship_type(&self, client_id: &Uuid, username: &str, page_size: i64, cursor: UserPaginationCursor) -> Result<Vec<UserWithRelationship>, Error> {
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
                WHERE
                    r_user.raw_name LIKE lower(concat('%', $1, '%'))
                    AND r_user.id <> $2
                    AND ($3 IS NULL OR (r_user.display_name, r_user.id) > ($3, $4))
                ORDER BY r_user.display_name ASC, r_user.id ASC
                LIMIT $5
            "#
        )
            .bind(username)
            .bind(client_id)
            .bind(cursor.last_seen_name)
            .bind(cursor.last_seen_id)
            .bind(page_size)
            .fetch_all(&self.pool).await?;
        Ok(user)
    }

    pub async fn select_open_friend_requests(&self, client_id: &Uuid) -> Result<Vec<User>, Error> {
        let requests = sqlx::query_as!(
            User,
            r#"SELECT
                u.id,
                u.display_name,
                u.profile_picture,
                u.street_credits,
                u.description,
                u.friends_count
                FROM app_user u
                INNER JOIN user_relationship ur ON
                    (ur.user_a_id = u.id AND ur.user_b_id = $1 AND ur.state = 'A_INVITED') OR
                    (ur.user_b_id = u.id AND ur.user_a_id = $1 AND ur.state = 'B_INVITED')
            "#,
            client_id
        ).fetch_all(&self.pool).await?;
        Ok(requests)
    }

    pub async fn find_users_with_specific_relationship(
        &self,
        client_id: &Uuid,
        state: RelationshipState,
    ) -> Result<Vec<User>, Error> {
        let users = sqlx::query_as!(
            User,
            r#"
                SELECT
                    u.id,
                    u.display_name,
                    u.profile_picture,
                    u.street_credits,
                    u.description,
                    u.friends_count
                FROM
                    app_user u
                INNER JOIN
                    user_relationship rl ON u.id = (
                        CASE
                            WHEN rl.user_a_id = $1 THEN rl.user_b_id
                            WHEN rl.user_b_id = $1 THEN rl.user_a_id
                            ELSE NULL
                        END
                    )
                WHERE
                    rl.state = $2
            "#,
            client_id,
            state.to_string()
        ).fetch_all(&self.pool).await?;
        Ok(users)
    }

    pub async fn search_for_relationship(&self, conn: &mut PgConnection, client_id: &Uuid, other_id: &Uuid) -> Result<Option<UserRelationship>, Error>
    {
        let relationship = sqlx::query_as!(
            UserRelationship,
            r#"
                SELECT
                    ur.user_a_id,
                    ur.user_b_id,
                    ur.state as "state: RelationshipState",
                    ur.relationship_change_timestamp
                FROM user_relationship ur
                    WHERE ur.user_a_id = $1 AND ur.user_b_id = $2 OR ur.user_b_id = $1 AND ur.user_a_id = $2
                FOR UPDATE
            "#,
            client_id,
            other_id
        ).fetch_optional(&mut *conn).await?;
        Ok(relationship)
    }

    pub async fn insert_relationship(&self, conn: &mut PgConnection, user_relationship: UserRelationship) -> Result<(), Error> {
            sqlx::query!(
            r#"
                INSERT INTO user_relationship (user_a_id, user_b_id, state, relationship_change_timestamp)
                VALUES ($1, $2, $3, $4)
            "#,
                user_relationship.user_a_id,
                user_relationship.user_b_id,
                user_relationship.state.to_string(),
                user_relationship.relationship_change_timestamp
            ).execute(&mut *conn).await?;
        Ok(())
    }

    pub async fn update_relationship_state(
        &self,
        conn: &mut PgConnection,
        user_a_id: &Uuid,
        user_b_id: &Uuid,
        new_state: RelationshipState,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
                UPDATE user_relationship
                    SET state = $1, relationship_change_timestamp = NOW()
                WHERE user_a_id = $2 AND user_b_id = $3
            "#,
            new_state.to_string(),
            user_a_id,
            user_b_id
        ).execute(&mut *conn).await?;
        Ok(())
    }

    pub async fn delete_relationship_state(
        &self,
        conn: &mut PgConnection,
        user_relationship: UserRelationship
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
                DELETE FROM user_relationship
                WHERE user_a_id = $1 AND user_b_id = $2
            "#,
            user_relationship.user_a_id,
            user_relationship.user_b_id
        ).execute(&mut *conn).await?;
        Ok(())
    }

    pub async fn increment_friends_count(
        &self,
        tx: &mut PgConnection,
        user_id: &Uuid,
    ) -> Result<(), Error> {
        sqlx::query!(
            r#"
                UPDATE app_user
                    SET friends_count = friends_count + 1
                WHERE id = $1
            "#,
            user_id
        ).execute(tx).await?;
        Ok(())
    }

    pub async fn decrement_friends_count(
        &self,
        tx: &mut PgConnection,
        user_id: &Uuid,
    ) -> Result<(), Error> {
        sqlx::query!(
            r#"
                UPDATE app_user
                    SET friends_count = friends_count - 1
                WHERE id = $1
            "#,
            user_id
        ).execute(tx).await?;
        Ok(())
    }

}