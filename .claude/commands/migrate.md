Erstelle eine neue SQLx-Migration für das ISM-Projekt.

Migrationsname (snake_case): $ARGUMENTS

Schritte:
1. Führe `sqlx migrate add $ARGUMENTS` aus — das erzeugt eine neue Datei in `migrations/`
2. Zeige mir den Pfad der neu erstellten Migrationsdatei
3. Warte auf meine SQL-Implementierung, bevor du weitermachst
4. Sobald ich die Migration befüllt habe und du sie anwenden sollst:
   - Führe `sqlx migrate run` aus
   - Führe danach `cargo sqlx prepare` aus, damit die Compile-Time-Metadaten aktuell sind
   - Weise mich darauf hin, dass `.sqlx/` committed werden muss