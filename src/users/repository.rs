use crate::core::{Database, Repository};
use crate::users::entity::{UserRelationshipRow, UserRow, UserWithRelationshipRow};
use crate::users::model::{RelationshipState, UserPaginationCursor};
use sqlx::{Error, PgConnection, query_as};
use uuid::Uuid;

/// Every column of `app_user` that [`UserRow`] decodes, aliased to `r_user`.
///
/// `app_user` belongs to the wider Meventure platform and carries columns ISM has no use for, so
/// the list is explicit rather than `SELECT *`. It is shared because four runtime-checked queries
/// must select exactly the same set: `UserRow::from_row` fails at *runtime* on a missing column, and
/// unlike the `query_as!` macro these queries get no compile-time column check.
///
/// A macro expanding to a string literal rather than a `const`, so the call sites can `concat!` it
/// into a `&'static str`. Building the SQL with `format!` would produce a `String`, which sqlx 0.9
/// rejects unless wrapped in `AssertSqlSafe` — an audit escape hatch that would be a lie here and a
/// bad precedent to set in a file full of user-supplied bind parameters.
macro_rules! user_columns {
    () => {
        r#"
            r_user.id,
            r_user.display_name,
            r_user.profile_picture,
            r_user.street_credits,
            r_user.description,
            r_user.friends_count,
            r_user.posts_count,
            r_user.role,
            r_user.email,
            r_user.created_at,
            r_user.deleted_at,
            r_user.last_modified_at,
            r_user.raw_name
        "#
    };
}

/// User profiles and the symmetric `user_relationship` table.
#[derive(Clone)]
pub struct UserRepository {
    db: Database,
}

impl Repository for UserRepository {
    fn new(db: &Database) -> Self {
        Self { db: db.clone() }
    }
}

impl UserRepository {
    pub async fn find_user_by_id_with_relationship_type(&self, client_id: &Uuid, searched_user_id: &Uuid) -> Result<Option<UserWithRelationshipRow>, Error> {
        let user = query_as::<_, UserWithRelationshipRow>(concat!(
            "SELECT ",
            user_columns!(),
            r#",
                user_relationship.user_a_id,
                user_relationship.user_b_id,
                user_relationship.state,
                user_relationship.relationship_change_timestamp
                FROM app_user r_user
                LEFT JOIN user_relationship ON
                    (user_relationship.user_a_id = r_user.id AND user_relationship.user_b_id = $2) OR
                    (user_relationship.user_b_id = r_user.id AND user_relationship.user_a_id = $2)
                WHERE r_user.id = $1 AND r_user.id <> $2
                  AND r_user.deleted_at IS NULL
            "#
        ))
        .bind(searched_user_id)
        .bind(client_id)
        .fetch_optional(self.db.pool())
        .await?;
        Ok(user)
    }

    /// The one user query written as a `query_as!` macro, so the column list below is verified
    /// against the live schema at build time. The runtime queries in this file cannot be, which
    /// is why they share [`USER_COLUMNS`] instead.
    pub async fn find_user_by_id(&self, user_id: &Uuid) -> Result<Option<UserRow>, Error> {
        let user = query_as!(
            UserRow,
            r#"SELECT
                    r_user.id,
                    r_user.display_name,
                    r_user.profile_picture,
                    r_user.street_credits,
                    r_user.description,
                    r_user.friends_count,
                    r_user.posts_count,
                    r_user.role,
                    r_user.email,
                    r_user.created_at,
                    r_user.deleted_at,
                    r_user.last_modified_at,
                    r_user.raw_name
                    FROM app_user r_user
                    WHERE r_user.id = $1
                "#,
            user_id
        )
        .fetch_optional(self.db.pool())
        .await?;
        Ok(user)
    }

    pub async fn find_user_by_name_with_relationship_type(
        &self,
        client_id: &Uuid,
        username: &str,
        page_size: i64,
        cursor: UserPaginationCursor,
    ) -> Result<Vec<UserWithRelationshipRow>, Error> {
        let user = query_as::<_, UserWithRelationshipRow>(concat!(
            "SELECT ",
            user_columns!(),
            r#",
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
                    AND r_user.deleted_at IS NULL
                    AND ($3 IS NULL OR (r_user.display_name, r_user.id) > ($3, $4))
                ORDER BY r_user.display_name ASC, r_user.id ASC
                LIMIT $5
            "#
        ))
        .bind(username)
        .bind(client_id)
        .bind(cursor.last_seen_name)
        .bind(cursor.last_seen_id)
        .bind(page_size)
        .fetch_all(self.db.pool())
        .await?;
        Ok(user)
    }

    /// Paginated incoming friend requests, ordered by display name. Optional
    /// case-insensitive name filter via the indexed `raw_name` column; keyset over
    /// `(display_name, id)`. Callers pass `limit = page_size + 1` to detect a next page.
    pub async fn select_open_friend_requests(
        &self,
        client_id: &Uuid,
        username: Option<&str>,
        cursor: UserPaginationCursor,
        limit: i64,
    ) -> Result<Vec<UserRow>, Error> {
        let requests = query_as::<_, UserRow>(concat!(
            "SELECT ",
            user_columns!(),
            r#"
                FROM app_user r_user
                INNER JOIN user_relationship ur ON
                    (ur.user_a_id = r_user.id AND ur.user_b_id = $1 AND ur.state = 'A_INVITED') OR
                    (ur.user_b_id = r_user.id AND ur.user_a_id = $1 AND ur.state = 'B_INVITED')
                WHERE
                    ($2::text IS NULL OR r_user.raw_name LIKE lower(concat('%', $2, '%')))
                    AND r_user.deleted_at IS NULL
                    AND ($3::text IS NULL OR (r_user.display_name, r_user.id) > ($3, $4))
                ORDER BY r_user.display_name ASC, r_user.id ASC
                LIMIT $5
            "#
        ))
        .bind(client_id)
        .bind(username)
        .bind(cursor.last_seen_name)
        .bind(cursor.last_seen_id)
        .bind(limit)
        .fetch_all(self.db.pool())
        .await?;
        Ok(requests)
    }

    /// Paginated list of users in a specific relationship state (e.g. friends),
    /// ordered by display name. Optional case-insensitive name filter via the
    /// indexed `raw_name`; keyset over `(display_name, id)`.
    pub async fn find_users_with_specific_relationship(
        &self,
        client_id: &Uuid,
        state: RelationshipState,
        username: Option<&str>,
        cursor: UserPaginationCursor,
        limit: i64,
    ) -> Result<Vec<UserRow>, Error> {
        let users = query_as::<_, UserRow>(concat!(
            "SELECT ",
            user_columns!(),
            r#"
                FROM
                    app_user r_user
                INNER JOIN
                    user_relationship rl ON r_user.id = (
                        CASE
                            WHEN rl.user_a_id = $1 THEN rl.user_b_id
                            WHEN rl.user_b_id = $1 THEN rl.user_a_id
                            ELSE NULL
                        END
                    )
                WHERE
                    rl.state = $2
                    AND r_user.deleted_at IS NULL
                    AND ($3::text IS NULL OR r_user.raw_name LIKE lower(concat('%', $3, '%')))
                    AND ($4::text IS NULL OR (r_user.display_name, r_user.id) > ($4, $5))
                ORDER BY r_user.display_name ASC, r_user.id ASC
                LIMIT $6
            "#
        ))
        .bind(client_id)
        .bind(state.to_string())
        .bind(username)
        .bind(cursor.last_seen_name)
        .bind(cursor.last_seen_id)
        .bind(limit)
        .fetch_all(self.db.pool())
        .await?;
        Ok(users)
    }

    pub async fn search_for_relationship(&self, conn: &mut PgConnection, client_id: &Uuid, other_id: &Uuid) -> Result<Option<UserRelationshipRow>, Error> {
        let relationship = sqlx::query_as!(
            UserRelationshipRow,
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
        )
        .fetch_optional(&mut *conn)
        .await?;
        Ok(relationship)
    }

    pub async fn insert_relationship(&self, conn: &mut PgConnection, user_relationship: &UserRelationshipRow) -> Result<(), Error> {
        sqlx::query!(
            r#"
                INSERT INTO user_relationship (user_a_id, user_b_id, state, relationship_change_timestamp)
                VALUES ($1, $2, $3, $4)
            "#,
            user_relationship.user_a_id,
            user_relationship.user_b_id,
            user_relationship.state.to_string(),
            user_relationship.relationship_change_timestamp
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    pub async fn update_relationship_state(
        &self,
        conn: &mut PgConnection,
        user_a_id: &Uuid,
        user_b_id: &Uuid,
        new_state: RelationshipState,
    ) -> Result<UserRelationshipRow, sqlx::Error> {
        let entity = sqlx::query_as!(
            UserRelationshipRow,
            r#"
                UPDATE user_relationship
                    SET state = $1, relationship_change_timestamp = NOW()
                WHERE user_a_id = $2 AND user_b_id = $3
                RETURNING
                    user_a_id,
                    user_b_id,
                    state as "state: RelationshipState",
                    relationship_change_timestamp
            "#,
            new_state.to_string(),
            user_a_id,
            user_b_id
        )
        .fetch_one(&mut *conn)
        .await?;
        Ok(entity)
    }

    pub async fn delete_relationship_state(&self, conn: &mut PgConnection, user_relationship: UserRelationshipRow) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
                DELETE FROM user_relationship
                WHERE user_a_id = $1 AND user_b_id = $2
            "#,
            user_relationship.user_a_id,
            user_relationship.user_b_id
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    pub async fn increment_friends_count(&self, tx: &mut PgConnection, user_id: &Uuid) -> Result<(), Error> {
        sqlx::query!(
            r#"
                UPDATE app_user
                    SET friends_count = friends_count + 1
                WHERE id = $1
            "#,
            user_id
        )
        .execute(tx)
        .await?;
        Ok(())
    }

    pub async fn decrement_friends_count(&self, tx: &mut PgConnection, user_id: &Uuid) -> Result<(), Error> {
        sqlx::query!(
            r#"
                UPDATE app_user
                    SET friends_count = friends_count - 1
                WHERE id = $1
            "#,
            user_id
        )
        .execute(tx)
        .await?;
        Ok(())
    }

    pub async fn find_blocked_relationships(&self, client_id: &Uuid, users_to_validate: &Vec<Uuid>) -> Result<Vec<Uuid>, Error> {
        let blocked_states_str: [&str; 3] = ["A_BLOCKED", "B_BLOCKED", "ALL_BLOCKED"];
        let blocked_states_string_vec: Vec<String> = blocked_states_str.map(String::from).to_vec();

        let blocked_users_optional: Vec<Option<Uuid>> = sqlx::query_scalar!(
            r#"
                SELECT user_b_id FROM user_relationship
                WHERE user_a_id = $1 AND user_b_id = ANY($2) AND state = ANY($3)
                UNION
                SELECT user_a_id FROM user_relationship
                WHERE user_b_id = $1 AND user_a_id = ANY($2) AND state = ANY($3)
            "#,
            client_id,
            users_to_validate,
            &blocked_states_string_vec
        )
        .fetch_all(self.db.pool())
        .await?;
        let blocked_users: Vec<Uuid> = blocked_users_optional.into_iter().flatten().collect();
        Ok(blocked_users)
    }
}
