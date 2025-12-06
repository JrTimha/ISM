# ISM - Instant Messenger for SaaS Backends

**ISM (Instant SaaS Messenger)** is a high-performance, scalable messaging solution specifically designed for SaaS backends. Written in Rust, it leverages Tokio and Axum to provide an asynchronous, stable, and efficient infrastructure.


> ✨ **If you have the same struggles as me to find an easy plug-and-play open source instant messaging solution, I would be happy if you contribute and use ISM!** 🙏


I am using ISM as my social backend in my own app, here you see an example video with ISM in action:

https://github.com/user-attachments/assets/e02c1ef1-c29d-438d-b77d-3346945ae74d


## Key Points of ISM:

-   **Scalability**: Built with the asynchronous Tokio runtime, ISM efficiently handles thousands of simultaneous connections.
-   **OAUTH2 & OIDC**: Supports JWT-based authentication via OpenID Connect (OIDC) Identity Providers (IDPs). (Currently tested only with Keycloak).
-   **Easy Integration**: Designed for seamless integration with existing SaaS architectures.
-   **Real-time Notifications**: Delivers messages in real-time using Server-Sent Events (SSE), typically achieving latency under 30ms.
-   **Persistent Storage**: Persists user and chat room metadata to a relational database, while storing room messages in a NoSQL database for optimal performance.
-   **Custom Notifications**: Send custom JSON payloads as notifications to your users via Kafka or Webhooks.
-   **S3 Support**: Functionality for uploading content to S3 Buckets is currently in progress.
-   **Event-Driven Chat Protocol**: ISM utilizes a simple, event-driven chat protocol. Clients query for the latest data via standard HTTP requests and receive real-time updates through an SSE connection. All data is exchanged in JSON format.

## Chat Functionalities:
-   **Room Types**: Supports private rooms (two users) and group rooms (up to 10 users).
-   **Message Types**: Handles media, text, system, and reply messages.
-   **Room Management**: Allows users to invite others to rooms and leave rooms.
-   **Chat History**: Provides support for scrolling through the entire chat timeline of a room.
-   **Multi-Device Support**: A single account can use an ISM chat client on multiple devices concurrently. Data is synchronized across devices thanks to the event-driven architecture.
-   **Read Status Tracking**: Tracks the read status for each user within a room, indicating which messages have been seen.


## Supported Database
-   **Message Storage (NoSQL):**
    -   **ScyllaDB**: Store message data in your ScyllaDB Cluster.
    -   **Apache Cassandra**: Store message data in your Apache Cassandra Cluster.
-   **User/Room Metadata Storage (Relational):**
    -   **PostgreSQL**: Retrieve and manage user and room metadata from one of these relational databases.

### Configure container environment

To configure the ISM container, you need to mount a configuration file named `production.config.toml` to the `/app` directory within the container.

These are the available configuration settings:

```toml
ism_url = "127.0.0.1" #Root URL, dont use localhost if you want to use IPv4 instead of IPv6
ism_port= 5403 #ISM is listening at this port
log_level = "info" # Logging level (e.g., "info", "debug", "warn", "error")
cors_origin = "http://localhost:4200" # Allowed CORS origin for frontend applications (wildcards are not supported)
use_kafka = false # Set to true to enable Kafka integration for custom notifications

[message_db_config] # Configuration for the NoSQL database (Cassandra/ScyllaDB)
db_url = "localhost:9042"
db_user = "cassandra"
db_password = "cassandra"
db_keyspace = "messaging"
with_db_init = true # Set to true to initialize required database tables on startup (use with caution in production)

[user_db_config] # Configuration for the relational database (PostgreSQL)
db_host = "localhost"
db_port = "32768"
db_user = "postgres"
db_password = "postgres"
db_name = "postgres"

[token_issuer]  # OIDC Identity Provider configuration
iss_host = "http://localhost:8180/" #Keycloak Root URL
iss_realm = "my-realm" #Keycloak Realm

[object_db_config] #connection to the S3 Bucket, used for images
db_user = "minioadmin"
db_url = "http://localhost:9000"
db_password = "minioadmin"
bucket_name = "meventure"


[kafka_config]  #OPTIONAL: Kafka configuration (only used if use_kafka = true)
bootstrap_host = "localhost"
bootstrap_port = 19192
topic = "user-notification-events"
client_id = "ism-1"
partition = [0]
consumer_group = "ism"

```
An example Docker Compose:

```yaml
services:
  ism:
    image: ghcr.io/jrtimha/ism:latest
    container_name: ism-container
    ports:
      - "5403:5403"
    environment:
      ISM_MODE: production
    volumes:
      - ./production.config.toml:/app/production.config.toml
```

Now go to `http://localhost:5403` in your browser, if everything works you will see: 

```
Hello, world! I'm your new ISM. 🤗
```

## ISM Endpoints:

Here's a summary of the available API endpoints:

*   **`GET /api/notify`**
    *   Handler: `poll_for_new_notifications`
    *   Description: Polls the server to check for new custom notifications for the authenticated user. This might be used as part of a long-polling strategy if SSE is not used or as a fallback.

*   **`GET /api/sse`**
    *   Handler: `stream_server_events`
    *   Description: Establishes a Server-Sent Events (SSE) connection. The client subscribing to this endpoint will receive real-time events pushed from the server (e.g., new messages, room updates, custom notifications).

*   **`POST /api/notify`**
    *   Handler: `add_notification`
    *   Description: Sends or triggers a custom notification targeted at specific users. Requires notification details (payload, target users/topic) in the request body.

*   **`POST /api/send-msg`**
    *   Handler: `send_message`
    *   Description: Sends a chat message to a specific room. Requires message details (room ID, content, type) in the request body.

*   **`POST /api/rooms/create-room`**
    *   Handler: `create_room`
    *   Description: Creates a new chat room (private or group). Requires room details (e.g., type, name, initial members) in the request body.

*   **`GET /api/rooms/{room_id}/users`**
    *   Handler: `get_users_in_room`
    *   Description: Retrieves a list of users who are members of the specified room (`{room_id}`).

*   **`GET /api/rooms/{room_id}/detailed`**
    *   Handler: `get_room_with_details`
    *   Description: Retrieves comprehensive details about a specific room (`{room_id}`), potentially including metadata, member list, and possibly recent messages.

*   **`GET /api/rooms/{room_id}/timeline`**
    *   Handler: `scroll_chat_timeline`
    *   Description: Fetches the message history (timeline) for a specific room (`{room_id}`). Likely supports pagination parameters (e.g., `?before_timestamp=...`, `?limit=...`) to load messages incrementally.

*   **`POST /api/rooms/{room_id}/mark-read`**
    *   Handler: `mark_room_as_read`
    *   Description: Marks messages in the specified room (`{room_id}`) as read by the authenticated user, likely up to the latest message or a specified point in time/message ID.

*   **`GET /api/rooms/{room_id}`**
    *   Handler: `get_room_list_item_by_id`
    *   Description: Retrieves basic information about a specific room (`{room_id}`), often used for displaying in a list of joined rooms (less detailed than the `/detailed` endpoint).

*   **`POST /api/rooms/{room_id}/leave`**
    *   Handler: `leave_room`
    *   Description: Allows the authenticated user to leave the specified room (`{room_id}`).

*   **`POST /api/rooms/{room_id}/invite/{user_id}`**
    *   Handler: `invite_to_room`
    *   Description: Invites a specific user (`{user_id}`) to join the specified room (`{room_id}`). Requires appropriate permissions.

*   **`GET /api/rooms`**
    *   Handler: `get_joined_rooms`
    *   Description: Retrieves a list of all rooms that the authenticated user is currently a member of.

## Development:

**Compiling the Project:** If you modify any SQL queries managed by sqlx, you must run `cargo sqlx prepare` before compiling the project to update the offline query data. This ensures compile-time checks for your SQL.

### Migrations
This project uses the sqlx-cli to handle migrations.
 
```
# Migrate up
sqlx migrate run

# Migrate down
sqlx migrate revert
```