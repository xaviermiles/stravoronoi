use sea_orm::Database;
use sea_orm::DatabaseConnection;
pub mod athlete;
pub mod run;

pub async fn connect_database() -> DatabaseConnection {
    Database::connect("sqlite::memory:").await.unwrap()
}
