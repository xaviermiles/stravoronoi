use sea_orm::ConnectOptions;
use sea_orm::Database;
use sea_orm::DatabaseConnection;
use sea_orm::DbErr;
pub mod athlete;
pub mod run;
pub mod session;

pub async fn connect_database() -> Result<DatabaseConnection, DbErr> {
    // An in-memory SQLite database lives inside a single connection. sea-orm/sqlx
    // opens a connection pool, so unless we pin the pool to a single connection,
    // `schema-sync` creates the tables on one connection while later queries hit a
    // different (empty) connection, causing "no such table" errors.
    // TODO: should move to a "proper" file-backed database on a mounted volume so there can be multiple connections.
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).min_connections(1);
    let database = Database::connect(options).await?;
    database
        .get_schema_registry("backend::models::*")
        .sync(&database)
        .await?;
    Ok(database)
}
