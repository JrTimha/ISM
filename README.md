# ISM - Instant Messenger for SaaS Backends

**ISM (Instant SaaS Messenger)** is a high-performance, scalable messaging solution specifically designed for SaaS backends. Written in Rust, it leverages Tokio and Axum to provide an asynchronous, stable, and efficient infrastructure.

## Supported Features

- **Scalability**: Handles millions of simultaneous connections using the asynchronous Tokio runtime.
- **Secure Communication**: Includes TLS support and JWT-based authentication.
- **Easy Integration**: Seamlessly integrates with existing SaaS architectures.
- **Modularity**: Easily extensible system with a clear modular architecture.
- **WebSocket Support**: Real-time message delivery with minimal latency.
- **Persistent Storage**: Supports both relational and NoSQL databases.

## Supported Database
- **ScyllaDB**: Saving all your messages in your ScyllaDB Cluster
- **Apache Cassandra**: Saving all your messages in your Apache Cassandra Cluster
- **MySQL || PostgreSQL**: Getting your user data from one of these databases

## Technologies

- **Rust**: For security, speed, and reliability.
- **Tokio**: Asynchronous runtime for high-performance network applications.
- **Axum**: Web framework for simple and flexible API development.
- **PostgreSQL/Redis**: Optional backend support for data persistence and caching.


### Prerequisites

- **Rust**: Version 1.80 or higher
- **Database**: PostgreSQL and either Apache Cassandra or ScyllaDB
