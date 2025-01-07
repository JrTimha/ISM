# ISM - Instant Messenger für SaaS-Backends

ISM (Instant SaaS Messenger) ist eine hochperformante, skalierbare Messaging-Lösung, die speziell für SaaS-Backends entwickelt wurde. Die Anwendung ist in **Rust** geschrieben und nutzt **Tokio** und **Axum**, um eine asynchrone, stabile und effiziente Infrastruktur bereitzustellen. 

## Features

- **Skalierbarkeit**: Unterstützt Millionen gleichzeitiger Verbindungen dank der asynchronen Tokio-Laufzeit.
- **Sichere Kommunikation**: TLS-Unterstützung und JWT-basierte Authentifizierung.
- **Einfach integrierbar**: Nahtlose Integration in bestehende SaaS-Architekturen.
- **Modularität**: Einfach erweiterbares System durch klare Modularchitektur.
- **WebSocket-Unterstützung**: Echtzeit-Nachrichtenübertragung mit minimaler Latenz.
- **Persistente Speicherung**: Unterstützung für relationale und NoSQL-Datenbanken.

## Technologien

- **Rust**: Für Sicherheit, Geschwindigkeit und Zuverlässigkeit.
- **Tokio**: Asynchrone Laufzeit für hochperformante Netzwerkanwendungen.
- **Axum**: Web-Framework für einfache und flexible API-Entwicklung.
- **Serde**: Für effiziente Serialisierung/Deserialisierung von JSON-Daten.
- **PostgreSQL/Redis**: Optionale Backend-Unterstützung für Datenpersistenz und Cache.

## Installation

### Voraussetzungen

- **Rust**: Version 1.80 oder höher
- **Datenbank**: PostgreSQL und MongoDB
- **OpenSSL**: Für TLS-Unterstützung
