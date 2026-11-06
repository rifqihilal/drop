//! Place search for the settings window, using Open-Meteo's free geocoding
//! service over WinHTTP. Only runs when the user presses "Cari".

use crate::lang::tr;
use crate::net::{self, percent_encode};

const HOST: &str = "geocoding-api.open-meteo.com";

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: String,
    /// "Kabupaten Sorong, Papua Barat Daya, Indonesia"
    pub region: String,
    pub latitude: f64,
    pub longitude: f64,
    pub elevation: f64,
}

/// Up to six places matching `query`, Indonesian ones first.
pub fn search(query: &str) -> Result<Vec<Place>, String> {
    let path = format!(
        "/v1/search?name={}&count=10&language=id&format=json",
        percent_encode(query.trim())
    );
    let body = net::get(HOST, &path).ok_or(tr(
        "Tidak dapat terhubung ke layanan pencarian",
        "Could not reach the search service",
    ))?;
    let mut places = parse(&body)?;
    // Prayer times follow the Kemenag method, so places in Indonesia come
    // first, and only they are listed when there are any.
    if places
        .iter()
        .any(|place| place.region.ends_with("Indonesia"))
    {
        places.retain(|place| place.region.ends_with("Indonesia"));
    }
    places.sort_by_key(|place| !place.region.ends_with("Indonesia"));
    places.truncate(6);
    Ok(places)
}

fn parse(body: &[u8]) -> Result<Vec<Place>, String> {
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|_| {
        tr(
            "Jawaban layanan tidak terbaca",
            "The search service gave no readable answer",
        )
        .to_string()
    })?;
    let results = json["results"].as_array().cloned().unwrap_or_default();
    Ok(results
        .iter()
        .filter_map(|result| {
            let text = |key: &str| result[key].as_str().unwrap_or("").to_string();
            let region = [text("admin2"), text("admin1"), text("country")]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            Some(Place {
                name: text("name"),
                region,
                latitude: result["latitude"].as_f64()?,
                longitude: result["longitude"].as_f64()?,
                elevation: result["elevation"].as_f64().unwrap_or(0.0).max(0.0),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_open_meteo_results() {
        let body = br#"{"results":[{"name":"Aimas","latitude":-0.95698,"longitude":131.33534,
            "elevation":9.0,"admin1":"Papua Barat Daya","admin2":"Kabupaten Sorong",
            "country":"Indonesia"}]}"#;
        let places = parse(body).unwrap();
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].name, "Aimas");
        assert_eq!(
            places[0].region,
            "Kabupaten Sorong, Papua Barat Daya, Indonesia"
        );
        assert_eq!(places[0].elevation, 9.0);
    }

    #[test]
    fn no_results_is_an_empty_list() {
        assert_eq!(parse(br#"{"generationtime_ms":0.1}"#).unwrap(), vec![]);
    }
}
