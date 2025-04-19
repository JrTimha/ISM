# ISM - Instant Messenger for SaaS Backends

**ISM (Instant SaaS Messenger)** is a high-performance, scalable messaging solution specifically designed for SaaS backends. Written in Rust, it leverages Tokio and Axum to provide an asynchronous, stable, and efficient infrastructure.

I am using ISM as my social backend in my own app, here you see an example video with ISM in action:

https://github.com/user-attachments/assets/e02c1ef1-c29d-438d-b77d-3346945ae74d


## Supported Features

- **Scalability**: Handles millions of simultaneous connections using the asynchronous Tokio runtime.
- **Secure Communication**: Includes TLS support and JWT-based authentication.
- **Easy Integration**: Seamlessly integrates with existing SaaS architectures.
- **Modularity**: Easily extensible system with a clear modular architecture.
- **Realtime Notification Support**: Real-time message delivery with minimal latency achieved with Server-Sent-Events (SSE).
- **Persistent Storage**:Persists users and chat rooms in a relational db and room messages to a NoSQL databases.

## Supported Database
- **ScyllaDB**: Saving all your messages in your ScyllaDB Cluster
- **Apache Cassandra**: Saving all your messages in your Apache Cassandra Cluster
- **MySQL || PostgreSQL**: Getting your user data from one of these databases

### Configure container environment

You need to mount a config file to the /app directory, file name must be production.config.toml!

These are the config settings:

```toml
ism_url = "localhost" #Root URL
ism_port= 5403 #ISM is listening at this port
log_level = "info"
cors_origin = "http://localhost:4200" #allowed origin, wildcards forbidden!

[message_db_config] #This is your cassandra db
db_url = "localhost:9042"
db_user = "cassandra"
db_password = "cassandra"
db_keyspace = "messaging"
with_db_init = true #initializing all database tables

[user_db_config] #This is your postgres database
db_host = "localhost"
db_port = "32768"
db_user = "postgres"
db_password = "postgres"
db_name = "postgres"

[token_issuer]
iss_host = "http://localhost:8180/" #Keycloak Root URL
iss_realm = "my-realm" #Keycloak Realm
```
An example Docker Compose:

```yaml
  ism:
    image: ghcr.io/jrtimha/ism:latest
    container_name: cassandra-container
    ports:
      - "5403:5403"
    environment:
      ISM_MODE: production
    volumes:
      - ./production.config.toml:/app/production.config.toml
```




