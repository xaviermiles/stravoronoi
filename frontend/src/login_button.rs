use crate::{BACKEND_BASE_URL, session};
use gloo_net::http::Request;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct LoginButtonProps {
    pub logged_in: bool,
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
    web_sys::window()
        .unwrap()
        .location()
        .reload()
        .unwrap();
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
        <button
            data-key="log-in"
            style="position: absolute; top: 10px; min-height: 40px; padding: 6px 16px; right: 10px; z-index: 1; border: 1px solid white; font-weight: 600; background-color: white; font-size: 14px; border-radius: 4px; font-family: \"Boathouse,Segoe UI,Helvetica Neue,-apple-system,system-ui,BlinkMacSystemFont,Roboto,Arial,sans-serif,Apple Color Emoji,Segoe UI Emoji,Segoe UI Symbol;\""
            onclick={onclick}>
            {button_text}
        </button>
    }
}
