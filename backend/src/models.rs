use sea_orm::DatabaseConnection;
use sea_orm::Database;



// TODO: move fetching of runs into backend so that map matching can be cached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub activity_id: i32,
    name: String,
    summary_map: Option<String>,
}

pub async fn connect_database() -> DatabaseConnection {
    Database::connect("sqlite::memory:").await.unwrap()
}
