use sea_orm::ConnectOptions;
use sea_orm::Database;
use sea_orm::DatabaseConnection;
use sea_orm::DbErr;
pub mod athlete;
pub mod run;
pub mod session;

const DATABASE_URL: &str = "sqlite://stravoronoi.db?mode=rwc";

/// Connect to the file-backed sqlite database.
pub async fn connect_database() -> Result<DatabaseConnection, DbErr> {
    let database = Database::connect(ConnectOptions::new(DATABASE_URL)).await?;
    database
        .get_schema_registry("backend::models::*")
        .sync(&database)
        .await?;
    Ok(database)
}
