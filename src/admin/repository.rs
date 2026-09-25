use diesel::{prelude::*, sql_types::Text};
use crate::{config::Config, error::AppError, repository::PoolConfig};
use super::model::{EditPolicy, Policy};

#[derive(QueryableByName)]
struct PolicyRow {
    #[diesel(sql_type = Text)]
    document: String,
}

pub fn load_policy(pool: &PoolConfig) -> Result<Policy, AppError> {
    let row = diesel::sql_query("SELECT document FROM admin_policy WHERE id = 1")
        .get_result::<PolicyRow>(&mut pool.conn()?)
        .optional()?;
    row.map_or(Ok(Policy::default()), |r| {
        serde_json::from_str(&r.document).map_err(|_| AppError::Internal)
    })
}
/// Read, merge, validate and persist under a SQLite write lock to avoid lost updates.
pub fn update_policy(
    pool: &PoolConfig,
    mut config: Config,
    edit: &EditPolicy,
) -> Result<(), AppError> {
    pool.conn()?.immediate_transaction(|conn| {
        let row = diesel::sql_query("SELECT document FROM admin_policy WHERE id = 1")
            .get_result::<PolicyRow>(conn).optional()?;
        if let Some(row) = row {
            let policy: Policy = serde_json::from_str(&row.document).map_err(|_| AppError::Internal)?;
            policy.apply(&mut config)?;
        }
        edit.apply(&mut config)?;
        let document = serde_json::to_string(&Policy::from_config(&config)).map_err(|_| AppError::Internal)?;
        diesel::sql_query("INSERT INTO admin_policy(id, document) VALUES(1, ?) ON CONFLICT(id) DO UPDATE SET document=excluded.document")
            .bind::<Text, _>(document).execute(conn)?;
        Ok(())
    })
}
