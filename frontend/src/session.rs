use gloo_storage::{LocalStorage, Storage, errors::StorageError};

const SESSION_ID_KEY: &str = "session_id";

pub fn get_session_id() -> Option<String> {
    match LocalStorage::get(SESSION_ID_KEY) {
        Ok(session_id) => Some(session_id),
        Err(StorageError::KeyNotFound(_)) => None,
        Err(err) => {
            // Log unexpected errors.
            log::info!("{}", err.to_string());
            None
        }
    }
}

pub fn set_session_id(session_id: String) {
    if let Err(err) = LocalStorage::set(SESSION_ID_KEY, &session_id) {
        log::warn!("Failed to set session ID: {}", err.to_string());
    };
}

pub fn delete_session_id() {
    log::info!("Deleting session ID.");
    LocalStorage::delete(SESSION_ID_KEY)
}

pub fn is_logged_in() -> bool {
    LocalStorage::get::<String>(SESSION_ID_KEY).is_ok()
}
