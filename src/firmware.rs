//! Firmware update check against Hollyland's cloud, the way HollyAudio does it.
//! HollyMic only tells you an update exists; it never downloads or flashes firmware.
//!
//! From HollyAudio 3.1.0 (`application/httpcommon/http/HttpRepository`, `HttpConst`):
//! - base URL `https://device-api.hollyland.com` (overseas build)
//! - `GET /api/v1/sign/getSignKey` → `{"code":200,"data":"<key>"}`
//! - `POST /api/v1/upgrade/checkBatchUpgrade` with headers `APP-PKG-CODE`, `APP-VERSION-VALUE`
//!   and body `{"sign","timestamp","body":[{"packageCode":"6301RX","versionValue":1000}, …]}`,
//!   where `sign = md5(key + timestamp + json(body) + key)` in lowercase hex.
//!   `data` is one entry per package, `null` when there is nothing newer.
//! - `POST /api/v1/upgrade/getBatchDownloadUrl` (same signing) gives `versionName` and notes,
//!   which the app compares with the unit's own version.

use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use md5::{Digest, Md5};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::protocol::{self, Model, Unit};
use crate::settings::Settings;
use crate::state::{Firmware, FirmwareUnit, Shared};

const BASE: &str = "https://device-api.hollyland.com";
/// HollyAudio's package name and version code (3.1.0), sent as headers like the app does.
const APP_PKG: &str = "com.hollyland.larkc1";
const APP_VERSION: &str = "3101020";
/// What `getSignKey` returned on 2026-11-13; used if that call fails.
const FALLBACK_KEY: &str = "hollyland-sign-key";
const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

/// HTTPS through the system `curl` (on every Mac and Linux desktop), so the app
/// carries no TLS stack of its own. Returns the HTTP status and the body.
fn curl(url: &str, headers: &[(&str, &str)], body: Option<&str>) -> Result<(u16, String)> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--max-time", "15", "-A", concat!("hollymic/", env!("CARGO_PKG_VERSION"))]);
    cmd.args(["-w", "\n%{http_code}"]);
    for (k, v) in headers {
        cmd.arg("-H").arg(format!("{k}: {v}"));
    }
    if body.is_some() {
        cmd.args(["-X", "POST", "--data-binary", "@-"]);
    }
    cmd.arg(url).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().context("running curl (is it installed?)")?;
    if let Some(b) = body {
        child.stdin.take().unwrap().write_all(b.as_bytes())?;
    } else {
        drop(child.stdin.take());
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("curl: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    Ok((code.trim().parse().unwrap_or(0), body.to_string()))
}

pub fn sign(key: &str, timestamp: u64, body_json: &str) -> String {
    let mut h = Md5::new();
    h.update(format!("{key}{timestamp}{body_json}{key}").as_bytes());
    hex::encode(h.finalize())
}

fn sign_key() -> String {
    let got = (|| -> Result<String> {
        let (_, text) = curl(&format!("{BASE}/api/v1/sign/getSignKey"), &[], None)?;
        let v: Value = serde_json::from_str(&text)?;
        v["data"].as_str().map(str::to_string).context("no sign key in reply")
    })();
    got.unwrap_or_else(|e| {
        log::debug!("getSignKey failed ({e:#}); using the known key");
        FALLBACK_KEY.to_string()
    })
}

fn post_signed(key: &str, path: &str, body: &Value) -> Result<Value> {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let body_json = serde_json::to_string(body)?;
    let request = format!("{{\"sign\":\"{}\",\"timestamp\":{ts},\"body\":{body_json}}}", sign(key, ts, &body_json));
    let headers = [("Content-Type", "application/json"), ("APP-PKG-CODE", APP_PKG), ("APP-VERSION-VALUE", APP_VERSION)];
    let (status, text) = curl(&format!("{BASE}{path}"), &headers, Some(&request))?;
    log::debug!("{path}: {status} {text}");
    let v: Value = serde_json::from_str(&text).with_context(|| format!("{path}: HTTP {status}, not JSON"))?;
    if v["code"].as_i64() != Some(200) {
        bail!("{path}: code {} {}", v["code"], v["msg"]);
    }
    Ok(v["data"].clone())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpgradeResponse {
    package_code: String,
    version_value: Option<i64>,
    /// Present in the live reply (2026-11-13), though HollyAudio's class doesn't read it.
    version_name: Option<String>,
    package_description_en: Option<String>,
    package_description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadResponse {
    package_code: String,
    version_name: Option<String>,
    package_description_en: Option<String>,
    package_description: Option<String>,
}

/// What the server offers per package code: (versionName, notes).
pub type Offers = Vec<(String, Option<String>, Option<String>)>;

/// Ask the server which packages have firmware. Empty = nothing offered.
pub fn query(codes: &[String]) -> Result<Offers> {
    let key = sign_key();
    let body: Vec<Value> = codes.iter().map(|c| json!({"packageCode": c, "versionValue": 1000})).collect();
    let data = post_signed(&key, "/api/v1/upgrade/checkBatchUpgrade", &Value::Array(body))?;
    let offered: Vec<UpgradeResponse> = serde_json::from_value::<Vec<Option<UpgradeResponse>>>(data)
        .context("checkBatchUpgrade reply")?
        .into_iter()
        .flatten()
        .collect();
    if offered.is_empty() {
        return Ok(Vec::new());
    }
    // The check reply already names the version; the download-URL call is the fallback.
    if offered.iter().all(|o| o.version_name.is_some()) {
        return Ok(offers_from(&offered, Vec::new()));
    }
    let body: Vec<Value> = offered
        .iter()
        .map(|o| json!({"packageCode": o.package_code, "versionValue": o.version_value.unwrap_or(0)}))
        .collect();
    let data = post_signed(&key, "/api/v1/upgrade/getBatchDownloadUrl", &Value::Array(body))?;
    let downloads: Vec<DownloadResponse> = serde_json::from_value::<Vec<Option<DownloadResponse>>>(data)
        .context("getBatchDownloadUrl reply")?
        .into_iter()
        .flatten()
        .collect();
    Ok(offers_from(&offered, downloads))
}

fn offers_from(offered: &[UpgradeResponse], downloads: Vec<DownloadResponse>) -> Offers {
    let mut out = Vec::new();
    for o in offered {
        let d = downloads.iter().find(|d| d.package_code == o.package_code);
        let name = o
            .version_name
            .clone()
            .or_else(|| d.and_then(|d| d.version_name.clone()))
            .map(|v| v.trim_start_matches(['v', 'V']).to_string());
        let nonempty = |v: &Option<String>| v.clone().filter(|t| !t.trim().is_empty());
        let notes = nonempty(&o.package_description_en)
            .or(nonempty(&o.package_description))
            .or_else(|| d.and_then(|d| nonempty(&d.package_description_en).or(nonempty(&d.package_description))));
        out.push((o.package_code.clone(), name, notes));
    }
    out
}

/// Compare what the server offers with the unit versions.
pub fn evaluate(model: &Model, rx: Option<&str>, tx: &[String], offers: &Offers) -> Firmware {
    let unit = |code: &str, versions: &[&str]| -> FirmwareUnit {
        match offers.iter().find(|(c, _, _)| c == code) {
            None => FirmwareUnit::default(),
            Some((_, name, notes)) => FirmwareUnit {
                latest: name.clone(),
                // No version name: the server offers a package, so assume it's an update.
                update: match name {
                    Some(n) => versions.iter().any(|v| protocol::compare_versions(n, v).is_gt()),
                    None => true,
                },
                notes: notes.clone(),
            },
        }
    };
    let rx_versions: Vec<&str> = rx.into_iter().collect();
    let tx_versions: Vec<&str> = tx.iter().map(String::as_str).collect();
    Firmware {
        checked_at: Some(SystemTime::now()),
        rx: unit(&protocol::package_code(model, Unit::Rx), &rx_versions),
        tx: unit(&protocol::package_code(model, Unit::Tx1), &tx_versions),
        error: None,
    }
}

/// Run one check for the current receiver and store the result.
pub fn check_now(shared: &Shared) -> Result<Firmware> {
    let s = shared.get();
    let model = s
        .model
        .as_deref()
        .and_then(|name| protocol::MODELS.iter().find(|m| m.name == name))
        .copied()
        .context("no receiver connected")?;
    let rx = s.receiver.version.clone();
    let tx: Vec<String> = s.mics.iter().filter_map(|m| m.info.version.clone()).collect();
    let codes = vec![protocol::package_code(&model, Unit::Rx), protocol::package_code(&model, Unit::Tx1)];
    let fw = match query(&codes) {
        Ok(offers) => evaluate(&model, rx.as_deref(), &tx, &offers),
        Err(e) => Firmware { checked_at: Some(SystemTime::now()), error: Some(format!("{e:#}")), ..Default::default() },
    };
    shared.update(|s| s.firmware = fw.clone());
    Ok(fw)
}

/// Background checker: once a day, and soon after a receiver or mic with a new version shows up.
pub fn spawn(shared: Shared, settings: Arc<RwLock<Settings>>) {
    std::thread::Builder::new()
        .name("firmware".into())
        .spawn(move || {
            let mut last_key = String::new();
            let mut last_check: Option<std::time::Instant> = None;
            loop {
                std::thread::sleep(Duration::from_secs(10));
                let s = shared.get();
                if !settings.read().unwrap().check_firmware || !s.connected() || s.demo {
                    continue;
                }
                let Some(rx) = s.receiver.version.clone() else { continue };
                let key = format!("{rx}|{:?}|{:?}", s.mics[0].info.version, s.mics[1].info.version);
                let due = last_check.is_none_or(|t| t.elapsed() > CHECK_EVERY);
                if key == last_key && !due {
                    continue;
                }
                last_key = key;
                last_check = Some(std::time::Instant::now());
                match check_now(&shared) {
                    Ok(fw) => log::info!(
                        "firmware check: rx update {} tx update {} {:?}",
                        fw.rx.update,
                        fw.tx.update,
                        fw.error
                    ),
                    Err(e) => log::info!("firmware check skipped: {e:#}"),
                }
            }
        })
        .expect("spawn firmware thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_like_the_app() {
        // md5("k" + "1" + "[]" + "k")
        assert_eq!(sign("k", 1, "[]"), hex::encode(Md5::digest(b"k1[]k")));
        assert_eq!(sign("k", 1, "[]").len(), 32);
    }

    #[test]
    fn nothing_offered_means_up_to_date() {
        let m = protocol::MODELS[0];
        let fw = evaluate(&m, Some("2.0.0.36"), &["2.1.0.36".into()], &Vec::new());
        assert!(!fw.update_available());
    }

    #[test]
    fn live_reply_with_version_names() {
        // checkBatchUpgrade reply from 2026-11-13 (trimmed): the M2 gets null, the M2S an offer.
        let data: Vec<Option<UpgradeResponse>> = serde_json::from_str(
            r#"[null,null,{"id":null,"packageCode":"6302RX","versionName":"V2.0.0.17","pkgLocaleDesc":"",
            "packageDescription":"1. Fixed known bugs","packageDescriptionEn":"","versionValue":20017,"fileUrl":null,
            "packageSize":732672,"upgradeType":0,"advised":0}]"#,
        )
        .unwrap();
        let offered: Vec<UpgradeResponse> = data.into_iter().flatten().collect();
        let offers = offers_from(&offered, Vec::new());
        assert_eq!(
            offers,
            vec![("6302RX".to_string(), Some("2.0.0.17".to_string()), Some("1. Fixed known bugs".to_string()))]
        );
    }

    #[test]
    fn newer_offer_is_an_update() {
        let m = protocol::MODELS[0];
        let data: Vec<Option<UpgradeResponse>> =
            serde_json::from_str(r#"[{"packageCode":"6301RX","versionValue":20040},null]"#).unwrap();
        let offered: Vec<UpgradeResponse> = data.into_iter().flatten().collect();
        let dl: Vec<DownloadResponse> = serde_json::from_str(
            r#"[{"packageCode":"6301RX","versionName":"V2.0.0.40","packageDescriptionEn":"Fixes"}]"#,
        )
        .unwrap();
        let offers = offers_from(&offered, dl);
        let fw = evaluate(&m, Some("2.0.0.36"), &[], &offers);
        assert!(fw.rx.update);
        assert_eq!(fw.rx.latest.as_deref(), Some("2.0.0.40"));
        assert!(!fw.tx.update);
        let fw = evaluate(&m, Some("2.0.0.40"), &[], &offers);
        assert!(!fw.rx.update, "same version is not an update");
    }
}
