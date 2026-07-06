use tower_sessions::MemoryStore;
use tower_sessions::service::SessionManagerLayer;

pub fn get_session_layer() -> SessionManagerLayer<MemoryStore> {
    // TODO: sign with key?
    let session_store = MemoryStore::default();
    SessionManagerLayer::new(session_store)
}
