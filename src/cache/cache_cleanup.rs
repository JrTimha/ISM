use std::time::Duration;
use redis::aio::{ConnectionManager};
use redis::{RedisResult};
use redis::{AsyncCommands};
use tracing::{debug, error};
use crate::cache::util::MASTER_INDEX_SET;

pub async fn periodic_cleanup_task(mut con: ConnectionManager) {

    let cleanup_interval = Duration::from_secs(3600); //atm each 1hr

    debug!("Starting Cache-Cleanup-Task.");

    loop {
        tokio::time::sleep(cleanup_interval).await;
        debug!("Starting periodic cache cleanup...");

        // getting all user ids from the master index set
        let user_ids: Vec<String> = match con.smembers(MASTER_INDEX_SET).await {
            Ok(ids) => ids,
            Err(e) => {
                error!("Error trying to get all users of the master cache index: {}", e);
                continue;
            }
        };

        for user_id in user_ids {
            if let Err(e) = cleanup_user_index(&mut con, &user_id).await {
                error!("Error trying to cleanup the notification cache of user {}: {}", user_id, e);
            }
        }
        debug!("Periodic cleanup finished.");
    }
}

async fn cleanup_user_index(
    con: &mut ConnectionManager,
    user_id: &str,
) -> RedisResult<()> {
    let sorted_set_key = format!("user_notifications:{}", user_id);

    // 1. getting all notification key references from the sorted set of the user
    let all_notification_keys: Vec<String> = con.zrange(&sorted_set_key, 0, -1).await?;

    if all_notification_keys.is_empty() {
        let _: isize = con.srem(MASTER_INDEX_SET, user_id).await?; //remove user from master index set if the set is empty
        return Ok(());
    }

    let mut keys_to_remove = Vec::new();

    // 2. Batch-Processing each key
    for chunk in all_notification_keys.chunks(100usize) {
        let mut pipe = redis::pipe();

        // Validate the existence of the key int he k/v store
        for key in chunk {
            pipe.exists(key);
        }
        let existence_flags: Vec<bool> = pipe.query_async(con).await?;

        // push keys to remove to a list
        for (key, exists) in chunk.iter().zip(existence_flags.iter()) {
            if !*exists {
                keys_to_remove.push(key);
            }
        }
    }

    // 5. Remove all keys without k/v reference from the sorted set of the user
    if !keys_to_remove.is_empty() {
        let count: isize = con.zrem(&sorted_set_key, keys_to_remove).await?;
        debug!("Cache cleanup for user {}: {} elements removed.", user_id, count);
    }

    Ok(())
}
