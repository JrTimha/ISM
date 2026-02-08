# ISM - Instant Messenger for SaaS Backends

[![Version](https://img.shields.io/badge/version-0.7.9-blue.svg)](https://github.com/JrTimha/ism)
[![License](https://img.shields.io/badge/license-AGPL%20v3-green.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)

**ISM (Instant SaaS Messenger)** is a high-performance, scalable messaging solution specifically designed for SaaS backends. Written in Rust, it leverages Tokio and Axum to provide an asynchronous, stable, and efficient infrastructure.

> ✨ **If you have the same struggles as me to find an easy plug-and-play open source instant messaging solution, I would be happy if you contribute and use ISM!** 🙏

I am using ISM as my social backend in my own app, here you see an example video with ISM in action:

https://github.com/user-attachments/assets/e02c1ef1-c29d-438d-b77d-3346945ae74d

## Table of Contents

- [Key Features](#key-features)
- [Chat Functionalities](#chat-functionalities)
- [Supported Databases](#supported-databases)
- [Quick Start](#quick-start)
- [Configuration](#configuration)
- [API Documentation](#api-documentation)
- [Development](#development)

## Key Features

-   **Scalability**: Built with the asynchronous Tokio runtime, ISM efficiently handles thousands of simultaneous connections.
-   **OAUTH2 & OIDC**: Supports JWT-based authentication via OpenID Connect (OIDC) Identity Providers (IDPs). (Currently tested only with Keycloak).
-   **Easy Integration**: Designed for seamless integration with existing SaaS architectures.
-   **Real-time Notifications**: Delivers messages in real-time using Server-Sent Events (SSE), typically achieving latency under 30ms.
-   **Persistent Storage**: Persists user and chat room metadata to a relational database, while storing room messages in a NoSQL database for optimal performance.
-   **Custom Notifications**: Send custom JSON payloads as notifications to your users via Kafka or Webhooks.
-   **S3 Support**: Upload room images and media content to S3-compatible object storage.
-   **Event-Driven Chat Protocol**: ISM utilizes a simple, event-driven chat protocol. Clients query for the latest data via standard HTTP requests and receive real-time updates through an SSE connection. All data is exchanged in JSON format.

## Chat Functionalities

-   **Room Types**: Supports private rooms (two users) and group rooms (multiple users).
-   **Message Types**: Handles text, media, reply, and room change messages.
-   **Room Management**: Allows users to create rooms, invite others, and leave rooms.
-   **Chat History**: Provides support for scrolling through the entire chat timeline of a room.
-   **Multi-Device Support**: A single account can use an ISM chat client on multiple devices concurrently. Data is synchronized across devices thanks to the event-driven architecture.
-   **Read Status Tracking**: Tracks the read status for each user within a room, indicating which messages have been seen.
-   **Friend System**: Built-in friend request system with accept/reject functionality.
-   **User Blocking**: Block/unblock users to prevent unwanted interactions.


## Supported Databases

-   **Message Storage (NoSQL):**
    -   **ScyllaDB**: Store message data in your ScyllaDB Cluster.
    -   **Apache Cassandra**: Store message data in your Apache Cassandra Cluster.
-   **User/Room Metadata Storage (Relational):**
    -   **PostgreSQL**: Retrieve and manage user and room metadata.
-   **Object Storage:**
    -   **S3-Compatible Storage**: MinIO, AWS S3, or any S3-compatible storage for media uploads.

## Quick Start

### Using Docker Compose

1. Create a `production.config.toml` configuration file (see [Configuration](#configuration) section below)

2. Create a `compose.yaml`:

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

3. Start the container:

```bash
docker compose up -d
```

4. Verify ISM is running by visiting `http://localhost:5403` - you should see:

```
Hello, world! I'm your new ISM. 🤗
```

## Configuration

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

## API Documentation

All API endpoints require authentication via JWT Bearer token in the `Authorization` header, except for the root endpoint and health check.

### Authentication

All protected endpoints require a valid JWT token from your OIDC provider (Keycloak):

```
Authorization: Bearer <your_jwt_token>
```

### Public Endpoints

#### Health Check
- **`GET /health`**
  - Returns server health status
  - **Response**: `200 OK` with "Healthy"

#### Root
- **`GET /`**
  - Returns welcome message
  - **Response**: `"Hello, world! I'm your new ISM. 🤗"`

---

### Real-Time Communication

#### Server-Sent Events (SSE)
- **`GET /api/sse`**
  - Establishes a Server-Sent Events connection for real-time updates
  - Client receives push notifications for new messages, room updates, and custom notifications
  - Connection stays open with keep-alive every 5 seconds
  - **Response**: Stream of SSE events containing JSON data

#### Get Notifications
- **`GET /api/notifications`**
  - Retrieves notification events since a specific timestamp
  - **Query Parameters**:
    - `timestamp` (DateTime): Retrieve notifications after this timestamp
  - **Response**: `200 OK` with array of notification objects

---

### Messaging

#### Send Message
- **`POST /api/send-msg`**
  - Sends a message to a chat room
  - **Request Body**:
    ```json
    {
      "chatRoomId": "uuid",
      "msgType": "Text|Media|Reply",
      "msgBody": {
        // For Text messages:
        "text": "string (1-4000 chars)",

        // For Media messages:
        "mediaUrl": "string (1-250 chars)",
        "mediaType": "string (1-80 chars)",
        "mimeType": "string (optional)",
        "altText": "string (optional)",

        // For Reply messages:
        "replyMsgId": "uuid",
        "replyCreatedAt": "datetime",
        "replyText": "string (1-4000 chars)"
      }
    }
    ```
  - **Response**: `200 OK` with created message object

---

### Room Management

#### Create Room
- **`POST /api/rooms/create-room`**
  - Creates a new chat room (Single or Group)
  - **Request Body**:
    ```json
    {
      "roomType": "Single|Group",
      "roomName": "string (optional, required for groups)",
      "invitedUsers": ["uuid", "uuid", ...]
    }
    ```
  - **Validation**:
    - Single rooms: exactly 2 users (sender + one other)
    - Group rooms: minimum 2 users
    - Sender must be in `invitedUsers` list
    - Blocked users are automatically filtered out
  - **Response**: `200 OK` with created room object

#### Get Joined Rooms
- **`GET /api/rooms`**
  - Retrieves all rooms the authenticated user is a member of
  - **Response**: `200 OK` with array of room objects

#### Get Room Details
- **`GET /api/rooms/{room_id}`**
  - Gets basic room information for display in room lists
  - **Path Parameters**:
    - `room_id` (UUID): Room identifier
  - **Response**: `200 OK` with room object

#### Get Room with Full Details
- **`GET /api/rooms/{room_id}/detailed`**
  - Gets comprehensive room information including all members
  - **Path Parameters**:
    - `room_id` (UUID): Room identifier
  - **Response**: `200 OK` with detailed room object including users array

#### Get Room Users
- **`GET /api/rooms/{room_id}/users`**
  - Lists all members of a specific room
  - **Path Parameters**:
    - `room_id` (UUID): Room identifier
  - **Response**: `200 OK` with array of user objects

#### Search for Existing Single Room
- **`GET /api/rooms/search`**
  - Checks if a single (private) room already exists with another user
  - **Query Parameters**:
    - `withUser` (UUID): The other user's ID
  - **Response**: `200 OK` with room UUID or `null`

#### Leave Room
- **`POST /api/rooms/{room_id}/leave`**
  - Removes the authenticated user from a room
  - **Path Parameters**:
    - `room_id` (UUID): Room identifier
  - **Response**: `200 OK`

#### Invite User to Room
- **`POST /api/rooms/{room_id}/invite/{user_id}`**
  - Invites a user to join a room
  - **Path Parameters**:
    - `room_id` (UUID): Room identifier
    - `user_id` (UUID): User to invite
  - **Response**: `200 OK`
  - **Error**: `403 Blocked` if user is blocked

#### Upload Room Image
- **`POST /api/rooms/{room_id}/upload-img`**
  - Uploads a room image/avatar to S3 storage
  - **Path Parameters**:
    - `room_id` (UUID): Room identifier
  - **Request**: `multipart/form-data` with `image` field
  - **Max Size**: 5MB
  - **Response**: `200 OK` with upload response containing URL

---

### Timeline & Messages

#### Get Room Timeline
- **`GET /api/rooms/{room_id}/timeline`**
  - Retrieves message history for a room with pagination
  - **Path Parameters**:
    - `room_id` (UUID): Room identifier
  - **Query Parameters**:
    - `timestamp` (DateTime): Load messages before this timestamp
  - **Response**: `200 OK` with array of message objects

#### Mark Room as Read
- **`POST /api/rooms/{room_id}/mark-read`**
  - Marks all messages in a room as read for the authenticated user
  - **Path Parameters**:
    - `room_id` (UUID): Room identifier
  - **Response**: `200 OK`

#### Get Read States
- **`GET /api/rooms/{room_id}/read-states`**
  - Retrieves read status for all members in a room
  - **Path Parameters**:
    - `room_id` (UUID): Room identifier
  - **Response**: `200 OK` with array of room member objects including read timestamps

---

### User Management

#### Search User by ID
- **`GET /api/users/{user_id}`**
  - Retrieves user profile and relationship status with authenticated user
  - **Path Parameters**:
    - `user_id` (UUID): User identifier
  - **Response**: `200 OK` with user object and relationship type

#### Search Users by Name
- **`GET /api/users/search`**
  - Searches for users by display name with pagination
  - **Query Parameters**:
    - `username` (string): Search query
    - `cursor` (string, optional): Pagination cursor for next page
  - **Response**: `200 OK` with results and next cursor

---

### Friend System

#### Get Friends
- **`GET /api/users/friends`**
  - Retrieves the authenticated user's friend list
  - **Response**: `200 OK` with array of user objects

#### Get Friend Requests
- **`GET /api/users/friends/requests`**
  - Retrieves pending friend requests received by the authenticated user
  - **Response**: `200 OK` with array of user objects

#### Send Friend Request
- **`POST /api/users/friends/add/{user_id}`**
  - Sends a friend request to another user
  - **Path Parameters**:
    - `user_id` (UUID): User to send request to
  - **Response**: `200 OK`

#### Accept Friend Request
- **`POST /api/users/friends/accept-request/{sender_id}`**
  - Accepts a pending friend request
  - **Path Parameters**:
    - `sender_id` (UUID): User who sent the request
  - **Response**: `200 OK`

#### Reject Friend Request
- **`DELETE /api/users/friends/reject-request/{sender_id}`**
  - Rejects a pending friend request
  - **Path Parameters**:
    - `sender_id` (UUID): User who sent the request
  - **Response**: `200 OK`

#### Remove Friend
- **`DELETE /api/users/friends/{friend_id}`**
  - Removes a user from friend list
  - **Path Parameters**:
    - `friend_id` (UUID): Friend to remove
  - **Response**: `200 OK`

---

### User Blocking

#### Block User
- **`POST /api/users/ignore/{user_id}`**
  - Blocks a user and automatically leaves any private room with them
  - **Path Parameters**:
    - `user_id` (UUID): User to block
  - **Response**: `200 OK` with updated relationship state

#### Unblock User
- **`DELETE /api/users/ignore/{user_id}`**
  - Unblocks a previously blocked user
  - **Path Parameters**:
    - `user_id` (UUID): User to unblock
  - **Response**: `200 OK` with updated relationship state

---

### Data Models

#### Message Types

- **Text**: Simple text message (1-4000 characters)
- **Media**: Link to media content (images, videos, etc.)
- **Reply**: Reply to another message
- **RoomChange**: System messages for user joined/left/invited events

#### Room Types

- **Single**: Private room between two users
- **Group**: Group room with multiple users

#### Relationship States

- **FRIEND**: Users are friends
- **INVITE_SENT**: Friend request sent by authenticated user
- **INVITE_RECEIVED**: Friend request received from other user
- **CLIENT_BLOCKED**: Authenticated user blocked the other user
- **CLIENT_GOT_BLOCKED**: Authenticated user was blocked by the other user

---

## Usage Examples

### Connecting to SSE Stream

```javascript
const token = "your_jwt_token";
const eventSource = new EventSource(`http://localhost:5403/api/sse`, {
  headers: {
    'Authorization': `Bearer ${token}`
  }
});

eventSource.onmessage = (event) => {
  const notification = JSON.parse(event.data);
  console.log('Received:', notification);
  // Handle new messages, room updates, etc.
};
```

### Creating a Room

```bash
curl -X POST http://localhost:5403/api/rooms/create-room \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "roomType": "Single",
    "invitedUsers": ["user-uuid-1", "user-uuid-2"]
  }'
```

### Sending a Text Message

```bash
curl -X POST http://localhost:5403/api/send-msg \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "chatRoomId": "room-uuid",
    "msgType": "Text",
    "msgBody": {
      "text": "Hello, World!"
    }
  }'
```

### Fetching Chat Timeline

```bash
curl -X GET "http://localhost:5403/api/rooms/{room_id}/timeline?timestamp=2024-01-01T00:00:00Z" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN"
```

### Searching for Users

```bash
curl -X GET "http://localhost:5403/api/users/search?username=john" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN"
```

---

## Development

### Prerequisites

- Rust 2024 edition or later
- PostgreSQL database
- ScyllaDB or Apache Cassandra cluster
- Keycloak or compatible OIDC provider
- (Optional) S3-compatible object storage
- (Optional) Kafka cluster

### Building from Source

1. Clone the repository:
```bash
git clone https://github.com/JrTimha/ism.git
cd ism
```

2. Set up environment variables (create a `.env` file):
```env
DATABASE_URL=postgresql://user:password@localhost:5432/dbname
```

3. Run database migrations:
```bash
cargo install sqlx-cli
sqlx migrate run
```

4. Build the project:
```bash
cargo build --release
```

5. Run the application:
```bash
cargo run --release
```

### Database Migrations

This project uses `sqlx-cli` for database schema management:

```bash
# Apply pending migrations
sqlx migrate run

# Revert last migration
sqlx migrate revert

# Create a new migration
sqlx migrate add <migration_name>
```

### Working with SQLx

If you modify any SQL queries in the code, you must prepare the offline query metadata:

```bash
cargo sqlx prepare
```

This generates compile-time checked query metadata in `.sqlx/` directory, ensuring type safety for your SQL queries.

### Project Structure

```
src/
├── broadcast/        # SSE and notification broadcasting
├── cache/           # Redis caching and pub/sub
├── core/            # Core application state and config
├── database/        # Database connections (Cassandra, PostgreSQL, S3)
├── errors.rs        # Error handling
├── kafka/           # Kafka integration for notifications
├── keycloak/        # JWT authentication and OIDC
├── messaging/       # Message handling and routes
├── model/           # Data models and DTOs
├── repository/      # Database repository layer
├── rooms/           # Room management and timeline
├── router.rs        # API route definitions
├── user_relationship/ # Friend system and user blocking
└── main.rs          # Application entry point
```

### Running Tests

```bash
cargo test
```

### Docker Build

Build your own Docker image:

```bash
docker build -t ism:latest .
```

---

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request. For major changes, please open an issue first to discuss what you would like to change.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/AmazingFeature`)
3. Commit your changes (`git commit -m 'Add some AmazingFeature'`)
4. Push to the branch (`git push origin feature/AmazingFeature`)
5. Open a Pull Request

---

## License

This project is licensed under the GNU Affero General Public License v3.0 (AGPL-3.0) - see the [LICENSE](LICENSE) file for details.

---

## Support

If you encounter any issues or have questions:

- Open an issue on [GitHub Issues](https://github.com/JrTimha/ism/issues)
- Check existing issues for solutions
- Contribute to the project!

---

## Roadmap

- [x] JWT/OIDC Authentication
- [x] Real-time messaging via SSE
- [x] Friend system
- [x] User blocking
- [x] S3 image uploads
- [ ] End-to-end encryption
- [ ] Voice/Video call signaling
- [ ] Message reactions
- [ ] Typing indicators
- [ ] Message search functionality
- [ ] Admin dashboard

---

**Built with ❤️ using Rust, Tokio, and Axum**