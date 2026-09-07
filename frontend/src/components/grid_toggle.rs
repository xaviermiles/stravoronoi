use crate::map::{self, MapRef};
use gloo_storage::LocalStorage;
use gloo_storage::Storage;
use web_sys::HtmlInputElement;
use yew::prelude::*;

/// Local-storage key holding whether the debug road-grid overlay is shown.
const SHOW_GRID_STORAGE_KEY: &str = "show_road_grid";

pub fn get_show_grid_storage_value() -> bool {
    LocalStorage::get::<bool>(SHOW_GRID_STORAGE_KEY).unwrap_or(false)
}

#[derive(Properties)]
pub struct GridToggleProps {
    /// Shared handle to the map whose grid overlay is toggled.
    pub map: MapRef,
}

// The map handle can't derive `PartialEq`, so compare by pointer identity.
impl PartialEq for GridToggleProps {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.map, &other.map)
    }
}

/// Checkbox that shows or hides the debug road-grid overlay.
#[function_component]
#[allow(non_snake_case)]
pub fn GridToggle(props: &GridToggleProps) -> Html {
    let grid_visible = use_state(get_show_grid_storage_value);
    let on_toggle = {
        let map = props.map.clone();
        let grid_visible = grid_visible.clone();
        Callback::from(move |e: Event| {
            let checked = e.target_unchecked_into::<HtmlInputElement>().checked();
            grid_visible.set(checked);
            if let Err(err) = LocalStorage::set(SHOW_GRID_STORAGE_KEY, checked) {
                log::error!("Failed to set 'show_road_grid' setting: {err}")
            };
            if let Some(map) = map.borrow().as_ref() {
                map::set_grid_visible(map, checked);
            }
        })
    };

    html! {
        <label id="grid-toggle">
            <input type="checkbox" checked={*grid_visible} onchange={on_toggle} />
            { "Show road grid\n(work in progress)" }
        </label>
    }
}
