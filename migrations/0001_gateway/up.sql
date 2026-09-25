CREATE TABLE sessions (token_hash TEXT PRIMARY KEY NOT NULL, identity TEXT NOT NULL, sid TEXT NOT NULL, issuer TEXT NOT NULL, tenant TEXT NOT NULL, subject TEXT NOT NULL, expires_at TEXT NOT NULL);
CREATE TABLE consumed_tickets (id TEXT PRIMARY KEY NOT NULL, expires_at TEXT NOT NULL);
CREATE TABLE revoked_sessions (id TEXT PRIMARY KEY NOT NULL);
CREATE TABLE registrations (id TEXT PRIMARY KEY NOT NULL, identity TEXT NOT NULL, manifest TEXT NOT NULL, expires_at TEXT NOT NULL);
CREATE INDEX registrations_identity ON registrations(identity);
CREATE TABLE accounts (id TEXT PRIMARY KEY NOT NULL, held_micros BIGINT NOT NULL CHECK (held_micros >= 0));
CREATE TABLE requests (id TEXT PRIMARY KEY NOT NULL, identity TEXT NOT NULL, tenant_account TEXT NOT NULL, user_account TEXT NOT NULL, reservation BIGINT NOT NULL CHECK (reservation >= 0), charged BIGINT, status TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f+00:00', 'now')));
CREATE INDEX requests_pending ON requests(status,created_at);
