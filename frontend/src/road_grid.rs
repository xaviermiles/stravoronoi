/// Bindings to road grid endpoints.
use crate::BACKEND_BASE_URL;
use geojson::FeatureCollection;
use gloo_net::http::Request;
use http::status::StatusCode;
use std::time::Duration;
use yew::platform::time;

const WAIT: Duration = Duration::from_millis(50);

async fn get_feature_collection(url: &str) -> FeatureCollection {
    let response = loop {
        match Request::get(url).send().await {
            Ok(response) => {
                if response.status() == StatusCode::NO_CONTENT {
                    time::sleep(WAIT).await;
                    continue;
                }
                break response;
            }
            Err(err) => {
                log::error!("Error getting all ways: {err}");
                return FeatureCollection::default();
            }
        }
    };
    response.json::<FeatureCollection>().await.unwrap()
}

pub async fn get_all_ways() -> FeatureCollection {
    get_feature_collection(&format!("{BACKEND_BASE_URL}/api/grid/ways")).await
}

pub async fn get_intersections() -> FeatureCollection {
    get_feature_collection(&format!("{BACKEND_BASE_URL}/api/grid/intersections")).await
}
