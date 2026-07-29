use crate::map::{self, MapRef};
use web_sys::HtmlInputElement;
use yew::prelude::*;

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
    let grid_visible = use_state(|| false);
    let on_toggle = {
        let map = props.map.clone();
        let grid_visible = grid_visible.clone();
        Callback::from(move |e: Event| {
            let checked = e.target_unchecked_into::<HtmlInputElement>().checked();
            grid_visible.set(checked);
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
