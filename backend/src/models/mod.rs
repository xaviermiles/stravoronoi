/// Database models.
use sea_orm::ConnectOptions;
use sea_orm::Database;
use sea_orm::DatabaseConnection;
use sea_orm::DbErr;
pub mod athlete;
pub mod grid_cell;
pub mod grid_node;
pub mod grid_way;
pub mod run;
use std::fs;
pub mod session;

const DATABASE_FILENAME: &str = "stravoronoi.db";

pub fn clean_database() {
    if let Err(err) = fs::remove_file(DATABASE_FILENAME) {
        tracing::error!("{err}");
    }
}

/// Connect to the file-backed sqlite database.
pub async fn connect_database() -> Result<DatabaseConnection, DbErr> {
    let database_url = format!("sqlite://{DATABASE_FILENAME}?mode=rwc");
    let database = Database::connect(ConnectOptions::new(database_url)).await?;
    database
        .get_schema_registry("backend::models::*")
        .sync(&database)
        .await?;
    Ok(database)
}
