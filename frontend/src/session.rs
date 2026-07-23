use gloo_net::http::RequestBuilder;
use gloo_storage::{LocalStorage, Storage, errors::StorageError};
use web_sys::RequestCredentials;
use yew::prelude::*;

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

fn is_logged_in() -> bool {
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

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub username: Option<AttrValue>,
    pub img_url: AttrValue,
}

/// The authentication state, plus callbacks to update it.
pub struct Auth {
    pub logged_in: bool,
    pub profile: Option<Profile>,
    /// Call once a session ID has been stored (e.g. after the OAuth callback).
    pub on_login: Callback<()>,
    /// Call when a request comes back unauthorised, to drop back to logged-out.
    pub on_unauthorized: Callback<()>,
}

/// Track the athlete's login state and fetch their profile picture URL from the
/// backend whenever they are logged in.
#[hook]
pub fn use_auth() -> Auth {
    let logged_in = use_state(is_logged_in);
    let profile = use_state(|| None::<Profile>);

    let on_unauthorized = {
        let logged_in = logged_in.clone();
        Callback::from(move |_| logged_in.set(false))
    };
    let on_login = {
        let logged_in = logged_in.clone();
        Callback::from(move |_| logged_in.set(true))
    };

    // Fetch the profile picture URL from the backend whenever we become logged in.
    {
        let profile = profile.clone();
        let logged_in = logged_in.clone();
        let is_logged_in = *logged_in;
        use_effect_with_deps(
            move |&is_logged_in| {
                if is_logged_in {
                    wasm_bindgen_futures::spawn_local(async move {
                        match crate::strava::load_profile().await {
                            Ok(athlete) => profile.set(Some(Profile {
                                username: athlete
                                    .username
                                    .map(|username| AttrValue::from(username)),
                                img_url: AttrValue::from(athlete.profile_url),
                            })),
                            Err(crate::strava::LoadError::Unauthorized) => logged_in.set(false),
                            Err(crate::strava::LoadError::Other(err)) => {
                                log::error!("Failed to load profile URL: {err}")
                            }
                        }
                    });
                } else {
                    profile.set(None);
                }
                || ()
            },
            is_logged_in,
        );
    }

    Auth {
        logged_in: *logged_in,
        profile: (*profile).clone(),
        on_login,
        on_unauthorized,
    }
}
