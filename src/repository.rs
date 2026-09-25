use chrono::{Duration, Utc};
use diesel::{
    prelude::*,
    r2d2::{ConnectionManager, Pool, PooledConnection, CustomizeConnection},
    connection::SimpleConnection,
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use crate::{
    error::AppError,
    model::{Manifest, Session, TicketClaims},
    schema::{accounts, consumed_tickets, registrations, requests, revoked_sessions, sessions},
};

/// Apply connection-local guarantees on every pooled SQLite connection.
#[derive(Debug)]
struct SqlitePragmas;
impl CustomizeConnection<SqliteConnection, diesel::r2d2::Error> for SqlitePragmas {
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), diesel::r2d2::Error> {
        conn.batch_execute(
            "PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;",
        )
        .map_err(diesel::r2d2::Error::QueryError)
    }
}

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!();
#[derive(Clone)]
pub struct PoolConfig {
    pool: Pool<ConnectionManager<SqliteConnection>>,
}
impl PoolConfig {
    pub fn new(url: &str) -> anyhow::Result<Self> {
        let config = Self::connect(url)?;
        config.migrate()?;
        Ok(config)
    }
    pub fn connect(path: &str) -> anyhow::Result<Self> {
        // Durable files only: separate pooled :memory: databases break accounting.
        anyhow::ensure!(
            !path.is_empty()
                && !path.contains("://")
                && !path.starts_with("file:")
                && path != ":memory:",
            "DATABASE_PATH must point to a local persistent SQLite file"
        );
        let mut initial = SqliteConnection::establish(path)?;
        initial.batch_execute("PRAGMA busy_timeout = 5000; PRAGMA journal_mode = WAL;")?;
        let pool = Pool::builder()
            .max_size(8)
            .connection_timeout(std::time::Duration::from_secs(6))
            .connection_customizer(Box::new(SqlitePragmas))
            .build(ConnectionManager::new(path))?;
        Ok(Self { pool })
    }
    pub fn migrate(&self) -> anyhow::Result<()> {
        self.conn()?
            .run_pending_migrations(MIGRATIONS)
            .map_err(|_| anyhow::anyhow!("migration failed"))?;
        Ok(())
    }
    pub(crate) fn conn(
        &self,
    ) -> Result<PooledConnection<ConnectionManager<SqliteConnection>>, AppError> {
        self.pool.get().map_err(|_| AppError::Internal)
    }
}
pub trait GatewayRepository {
    fn check_ready(&self) -> Result<(), AppError>;
    fn exchange(&self, claims: &TicketClaims, session: &Session) -> Result<(), AppError>;
    fn authenticate(&self, hash: &str) -> Result<Session, AppError>;
    fn revoke(&self, session: &Session) -> Result<(), AppError>;
    fn register(&self, id: &str, session: &Session, manifest: &Manifest) -> Result<(), AppError>;
    fn manifest(&self, id: &str, identity: &str) -> Result<Manifest, AppError>;
    fn reserve(
        &self,
        id: &str,
        session: &Session,
        amount: i64,
        tenant_limit: i64,
        user_limit: i64,
    ) -> Result<(), AppError>;
    fn settle(&self, id: &str, charge: i64) -> Result<(), AppError>;
}
impl GatewayRepository for PoolConfig {
    fn check_ready(&self) -> Result<(), AppError> {
        let mut conn = self
            .pool
            .get_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| AppError::Internal)?;
        // A real schema query detects a reachable but unmigrated database.
        sessions::table
            .select(sessions::token_hash)
            .limit(1)
            .load::<String>(&mut conn)?;
        Ok(())
    }
    fn exchange(&self, claims: &TicketClaims, session: &Session) -> Result<(), AppError> {
        self.conn()?.immediate_transaction(|conn| {
            if revoked_sessions::table
                .find(claims.session_key())
                .count()
                .get_result::<i64>(conn)?
                > 0
            {
                return Err(AppError::Authentication);
            }
            diesel::insert_into(consumed_tickets::table)
                .values((
                    consumed_tickets::id
                        .eq(serde_json::json!([claims.iss, claims.jti]).to_string()),
                    consumed_tickets::expires_at.eq(session.expires_at),
                ))
                .execute(conn)?;
            diesel::insert_into(sessions::table)
                .values(session)
                .execute(conn)?;
            Ok(())
        })
    }
    fn authenticate(&self, hash: &str) -> Result<Session, AppError> {
        let mut conn = self.conn()?;
        let session = sessions::table
            .find(hash)
            .filter(sessions::expires_at.gt(Utc::now()))
            .select(Session::as_select())
            .first::<Session>(&mut conn)
            .optional()?
            .ok_or(AppError::Authentication)?;
        if revoked_sessions::table
            .find(&session.sid)
            .count()
            .get_result::<i64>(&mut conn)?
            > 0
        {
            return Err(AppError::Authentication);
        }
        Ok(session)
    }
    fn revoke(&self, session: &Session) -> Result<(), AppError> {
        diesel::insert_into(revoked_sessions::table)
            .values(revoked_sessions::id.eq(&session.sid))
            .on_conflict_do_nothing()
            .execute(&mut self.conn()?)?;
        Ok(())
    }
    fn register(&self, id: &str, session: &Session, manifest: &Manifest) -> Result<(), AppError> {
        diesel::insert_into(registrations::table)
            .values((
                registrations::id.eq(id),
                registrations::identity.eq(&session.identity),
                registrations::manifest
                    .eq(serde_json::to_string(manifest).map_err(|_| AppError::Invalid)?),
                registrations::expires_at.eq(Utc::now() + Duration::hours(24)),
            ))
            .execute(&mut self.conn()?)?;
        Ok(())
    }
    fn manifest(&self, id: &str, identity: &str) -> Result<Manifest, AppError> {
        let value = registrations::table
            .find(id)
            .filter(registrations::identity.eq(identity))
            .filter(registrations::expires_at.gt(Utc::now()))
            .select(registrations::manifest)
            .first::<String>(&mut self.conn()?)
            .optional()?
            .ok_or(AppError::Invalid)?;
        serde_json::from_str(&value).map_err(|_| AppError::Internal)
    }
    fn reserve(
        &self,
        id: &str,
        session: &Session,
        amount: i64,
        tenant_limit: i64,
        user_limit: i64,
    ) -> Result<(), AppError> {
        if amount <= 0 {
            return Err(AppError::Invalid);
        }
        let day = Utc::now().date_naive().to_string();
        let tenant = serde_json::json!([session.issuer, session.tenant, day]).to_string();
        let user =
            serde_json::json!([session.issuer, session.tenant, session.subject, day]).to_string();
        self.conn()?.immediate_transaction(|conn| {
            diesel::insert_into(requests::table)
                .values((
                    requests::id.eq(id),
                    requests::identity.eq(&session.identity),
                    requests::tenant_account.eq(&tenant),
                    requests::user_account.eq(&user),
                    requests::reservation.eq(amount),
                    requests::status.eq("pending"),
                ))
                .execute(conn)?;
            // BEGIN IMMEDIATE serializes writers before reading either budget.
            // Both account updates and the request record commit or roll back together.
            for (id, limit) in [(&tenant, tenant_limit), (&user, user_limit)] {
                diesel::insert_into(accounts::table)
                    .values((accounts::id.eq(id), accounts::held_micros.eq(0_i64)))
                    .on_conflict_do_nothing()
                    .execute(conn)?;
                let held = accounts::table
                    .find(id)
                    .select(accounts::held_micros)
                    .first::<i64>(conn)?;
                let next = held.checked_add(amount).ok_or(AppError::Budget)?;
                if next > limit {
                    return Err(AppError::Budget);
                }
                diesel::update(accounts::table.find(id))
                    .set(accounts::held_micros.eq(next))
                    .execute(conn)?;
            }
            Ok(())
        })
    }
    fn settle(&self, id: &str, charge: i64) -> Result<(), AppError> {
        if charge < 0 {
            return Err(AppError::Invalid);
        }
        self.conn()?.immediate_transaction(|conn| {
            let (tenant, user, reservation, status) = requests::table
                .find(id)
                .select((
                    requests::tenant_account,
                    requests::user_account,
                    requests::reservation,
                    requests::status,
                ))
                .first::<(String, String, i64, String)>(conn)?;
            if status != "pending" {
                return Err(AppError::Conflict);
            }
            for account in [tenant, user] {
                let held = accounts::table
                    .find(&account)
                    .select(accounts::held_micros)
                    .first::<i64>(conn)?;
                let next = held
                    .checked_sub(reservation)
                    .and_then(|n| n.checked_add(charge))
                    .ok_or(AppError::Internal)?;
                diesel::update(accounts::table.find(&account))
                    .set(accounts::held_micros.eq(next))
                    .execute(conn)?;
            }
            diesel::update(requests::table.find(id))
                .set((requests::charged.eq(charge), requests::status.eq("settled")))
                .execute(conn)?;
            Ok(())
        })
    }
}
