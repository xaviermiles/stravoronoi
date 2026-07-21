use gloo_net::http::RequestBuilder;
use gloo_storage::{LocalStorage, Storage, errors::StorageError};
use web_sys::RequestCredentials;

const SESSION_ID_KEY: &str = "session_id";

fn get_session_id() -> Option<String> {
    match LocalStorage::get(SESSION_ID_KEY) {
        Ok(session_id) => Some(session_id),
        Err(StorageError::KeyNotFound(_)) => None,
        Err(err) => {
            // Log unexpected errors.
            log::info!("{err}");
            None
        }
    }
}

pub fn set_session_id(session_id: String) {
    if let Err(err) = LocalStorage::set(SESSION_ID_KEY, &session_id) {
        log::warn!("Failed to set session ID: {err}");
    };
}

pub fn delete_session_id() {
    LocalStorage::delete(SESSION_ID_KEY)
}

pub fn is_logged_in() -> bool {
    LocalStorage::get::<String>(SESSION_ID_KEY).is_ok()
}

/// Return the builder authorised with the current session ID, or None if there is no current session ID.
pub fn authed(builder: RequestBuilder) -> Option<RequestBuilder> {
    let session_id = get_session_id()?;
    Some(
        builder
            .header("Authorization", &format!("Bearer {session_id}"))
            .credentials(RequestCredentials::Include),
    )
}
