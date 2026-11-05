//! Asks GitHub for Drop's latest release, when the About page's button is
//! pressed. The call blocks, so it runs on a worker thread.

use crate::net;

/// The running version, as in `Cargo.toml`.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Where a newer release is downloaded.
pub const RELEASES: &str = "https://github.com/rifqihilal/drop/releases/latest";

/// The latest release's version, such as "0.2.0"; None when GitHub can't be
/// reached or its answer has no tag.
pub fn latest() -> Option<String> {
    let body = net::get("api.github.com", "/repos/rifqihilal/drop/releases/latest")?;
    tag(&String::from_utf8_lossy(&body))
}

/// The release's tag from GitHub's JSON, without its leading "v".
fn tag(json: &str) -> Option<String> {
    let key = "\"tag_name\"";
    let rest = &json[json.find(key)? + key.len()..];
    let rest = &rest[rest.find('"')? + 1..];
    let value = &rest[..rest.find('"')?];
    Some(value.trim_start_matches(['v', 'V']).to_string())
}

/// Whether `latest` is a later version than `current`, part by part.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parts = |version: &str| -> Vec<u32> {
        version
            .split('.')
            .map(|part| part.trim().parse().unwrap_or(0))
            .collect()
    };
    let (latest, current) = (parts(latest), parts(current));
    let len = latest.len().max(current.len());
    let at = |parts: &[u32], i: usize| parts.get(i).copied().unwrap_or(0);
    (0..len)
        .map(|i| at(&latest, i).cmp(&at(&current, i)))
        .find(|order| order.is_ne())
        .is_some_and(|order| order.is_gt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_tag() {
        let json = r#"{"url":"x","tag_name": "v0.2.0","name":"Drop 0.2.0"}"#;
        assert_eq!(tag(json).as_deref(), Some("0.2.0"));
        assert_eq!(tag(r#"{"message":"Not Found"}"#), None);
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.10.0", "0.10.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
    }
}
