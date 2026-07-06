use sea_orm::Database;
use sea_orm::DatabaseConnection;
use sea_orm::DbErr;
pub mod athlete;
pub mod run;

pub async fn connect_database() -> Result<DatabaseConnection, DbErr> {
    let database = Database::connect("sqlite::memory:").await?;
    database
        .get_schema_registry("backend::models::*")
        .sync(&database)
        .await?;
    Ok(database)
}
