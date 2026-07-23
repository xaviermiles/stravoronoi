use crate::{BACKEND_BASE_URL, session};
use gloo_net::http::Request;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct LoginButtonProps {
    pub logged_in: bool,
    /// The athlete's profile picture URL. Only populated when logged in.
    #[prop_or_default]
    pub profile_url: Option<AttrValue>,
}

async fn logout() {
    if let Err(err) = session::authed(Request::post(&format!("{BACKEND_BASE_URL}/auth/logout")))
        .expect("clicking logout requires a session ID")
        .send()
        .await
    {
        log::error!("Error while logging out: {err}");
    }
    session::delete_session_id();
    web_sys::window().unwrap().location().reload().unwrap();
}

#[function_component]
#[allow(non_snake_case)]
pub fn LoginButton(props: &LoginButtonProps) -> Html {
    let button_text = if props.logged_in { "Log out" } else { "Log in" };
    let onclick = if props.logged_in {
        Callback::from(move |_| {
            wasm_bindgen_futures::spawn_local(logout());
        })
    } else {
        Callback::from(move |_| {
            web_sys::window()
                .unwrap()
                .location()
                .set_href(&format!("{BACKEND_BASE_URL}/auth/login"))
                .unwrap();
        })
    };
    html! {
        <div>
            <button data-key="log-in" onclick={onclick}>
                {button_text}
            </button>
            if let Some(url) = &props.profile_url {
                <img id="user-icon" src={url.clone()} alt="Profile picture" />
            }
        </div>
    }
}
