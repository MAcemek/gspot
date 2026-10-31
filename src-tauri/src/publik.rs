//! publik API: the zero-setup AI option. Provisioning (`POST /installs` with
//! the app token baked into this build), the per-app credential file, the
//! wallet, and the live status the panel renders (balance line, the one
//! justification sentence, the plan button, a 402 notice with exactly one link).
//!
//! Contract: ~/publik-api-research/CONTRACT.md §1, §3.2, §12, §13 and
//! docs/publik-sdk-convention.md §6. Copy rule: "publik API", dollars, never
//! tokens, never "credits" as a unit, never the provider's name.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://publikhq.com/api/v1";
pub const DASHBOARD_URL: &str = "https://publikhq.com/dashboard/api";
pub const TERMS_URL: &str = "https://publikhq.com/terms#api";
pub const APP_SLUG: &str = "gspot";
pub const DISCLOSURE_VERSION: u32 = 1;
/// The alias the gateway maps to its balanced tier: the writing quality the
/// sidebar features need. Anonymous keys cannot use `publik-smart`.
pub const DEFAULT_MODEL: &str = "publik-balanced";
/// Baked in by scripts/build-macos.sh (or `PUBLIK_APP_TOKEN` at `cargo build`).
/// A build without it has no publik option; your own key keeps working.
const APP_TOKEN: Option<&str> = option_env!("PUBLIK_APP_TOKEN");
/// The only host a claim / top-up link may point at. Anything else is dropped.
const LINK_HOST_PREFIX: &str = "https://publikhq.com/";

/// The one-sentence justification (CONTRACT §13 wording), shown wherever money is mentioned.
pub const WHY_IT_COSTS: &str = "The model's provider charges per use. publik charges a fixed, published price that sits above what the model costs publik; the difference keeps publik running and pays the developer who built this app. Nothing is charged behind your back, and every call is visible on your publik dashboard.";
/// The consent sheet's cost sentence (never promises a starter).
pub const DISCLOSURE_COST: &str = "A new computer starts at $0.00. Linking this computer to your publik account gives $0.05 of free use, once. After that every request is priced per use at a fixed, published price from your publik balance, and every charge is visible on your dashboard.";
pub const DISCLOSURE_DATA: &str = "The text you check goes through publik's servers to a shared model account. publik never trains on it and does not store it. Spelling and grammar checks stay on this Mac.";
pub const CTA_SIGN_IN: &str = "Sign in with publik";
pub const CTA_ADD_PLAN: &str = "Add a plan or pack";
pub const CTA_LATER: &str = "Later";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Credential {
    pub key: String,
    pub base_url: String,
    pub install_id: String,
    pub app_slug: String,
    pub model: String,
    pub claim_url: String,
    pub add_credit_url: String,
    pub claim_state: String,
    pub minted_at: String,
}

/// A typed gateway failure: what to show, and the one link to render.
#[derive(Clone, Debug, Serialize)]
pub struct PublikError {
    pub status: u16,
    pub kind: String,
    pub message: String,
    pub link_label: String,
    pub link_url: Option<String>,
    pub retry_after_secs: Option<u64>,
    pub reprovision: bool,
}

impl std::fmt::Display for PublikError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for PublikError {}

/// Live state from the last gateway answer (headers, 402s, transport errors).
#[derive(Clone, Debug, Default, Serialize)]
pub struct Live {
    pub balance_micros: Option<i64>,
    pub last_charge_micros: Option<i64>,
    pub claim_state: Option<String>,
    pub week_used_micros: Option<i64>,
    pub week_budget_micros: Option<i64>,
    pub model_served: Option<String>,
    pub exhausted: bool,
    pub disconnected: bool,
    pub unreachable: bool,
    pub notice: Option<Notice>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Notice {
    pub kind: String,
    pub message: String,
    pub link_label: String,
    pub link_url: String,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

fn live() -> Live {
    LIVE.lock().unwrap().clone().unwrap_or_default()
}
fn with_live(f: impl FnOnce(&mut Live)) {
    let mut g = LIVE.lock().unwrap();
    let mut l = g.clone().unwrap_or_default();
    f(&mut l);
    *g = Some(l);
}
pub fn clear_live() {
    *LIVE.lock().unwrap() = None;
}
pub fn dismiss_notice() {
    with_live(|l| l.notice = None);
}

pub fn can_provision() -> bool {
    APP_TOKEN.map(|t| !t.trim().is_empty()).unwrap_or(false)
}

pub fn publik_link(s: Option<&str>) -> Option<String> {
    let s = s?.trim();
    if s.starts_with(LINK_HOST_PREFIX) {
        Some(s.to_string())
    } else {
        None
    }
}

fn trim_url(s: &str) -> Option<String> {
    let t = s.trim().trim_end_matches('/');
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// `PUBLIK_API_BASE_URL` (dev) → the provisioning response's `base_url` → the default.
pub fn resolve_base_url(from_credential: &str) -> String {
    if let Some(u) = std::env::var("PUBLIK_API_BASE_URL").ok().and_then(|s| trim_url(&s)) {
        return u;
    }
    trim_url(from_credential).unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// `~/Library/Application Support/publik/apps` on macOS (the convention folder
/// Iris and the other publik apps share); `%APPDATA%\publik\apps` elsewhere.
fn apps_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = PathBuf::from(std::env::var("HOME").ok()?).join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let base = dirs::data_dir()?;
    Some(base.join("publik").join("apps"))
}

pub fn credential_path() -> Option<PathBuf> {
    Some(apps_dir()?.join(format!("{APP_SLUG}.json")))
}

/// Credential file written by builds that shipped under the old slug `grammarlama`.
fn legacy_credential_path() -> Option<PathBuf> {
    Some(apps_dir()?.join("grammarlama.json"))
}

/// The key in force: `PUBLIK_API_KEY` in the environment, else the credential file.
pub fn read_credential() -> Option<Credential> {
    if let Ok(k) = std::env::var("PUBLIK_API_KEY") {
        let k = k.trim().to_string();
        if k.starts_with("pk_") {
            return Some(Credential { key: k, base_url: resolve_base_url(""), model: DEFAULT_MODEL.into(), app_slug: APP_SLUG.into(), ..Default::default() });
        }
    }
    let path = credential_path()?;
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        // A computer set up by a build that shipped under the old slug: adopt its
        // credential once (same key, same install) and keep it at the new path.
        Err(_) => {
            let r = std::fs::read_to_string(legacy_credential_path()?).ok()?;
            if let Ok(mut c) = serde_json::from_str::<Credential>(&r) {
                c.app_slug = APP_SLUG.into();
                let _ = write_credential(&c);
            }
            r
        }
    };
    let mut c: Credential = serde_json::from_str(&raw).ok()?;
    if !c.key.starts_with("pk_") {
        return None;
    }
    c.app_slug = APP_SLUG.into();
    c.base_url = resolve_base_url(&c.base_url);
    if c.model.trim().is_empty() {
        c.model = DEFAULT_MODEL.into();
    }
    Some(c)
}

/// Owner-only on Unix (0600 file, 0700 folder). On Windows the file sits in the
/// user's own profile folder, which other accounts cannot read by default.
pub fn write_credential(c: &Credential) -> anyhow::Result<()> {
    let path = credential_path().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let json = serde_json::to_string_pretty(c)?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&path)?;
    use std::io::Write;
    f.write_all(json.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn forget_credential() {
    if let Some(p) = credential_path() {
        let _ = std::fs::remove_file(p);
    }
    if let Some(p) = legacy_credential_path() {
        let _ = std::fs::remove_file(p);
    }
    clear_live();
}

/// 16 random bytes as a v4 UUID string (the mint's idempotency key).
pub fn new_install_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}{}{}{}-{}{}-{}{}-{}{}-{}{}{}{}{}{}", h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15])
}

fn os_version() -> Option<String> {
    let out = std::process::Command::new("sw_vers").arg("-productVersion").output().ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() { None } else { Some(v) }
}

fn device_name() -> Option<String> {
    let out = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output().ok()?;
    let v: String = String::from_utf8_lossy(&out.stdout).trim().chars().take(120).collect();
    if v.is_empty() { None } else { Some(v) }
}

fn arch_name() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

fn header_str(h: &reqwest::header::HeaderMap, k: &str) -> Option<String> {
    h.get(k).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}
fn header_i64(h: &reqwest::header::HeaderMap, k: &str) -> Option<i64> {
    header_str(h, k).and_then(|s| s.parse::<i64>().ok())
}

fn err_field<'a>(v: &'a Value, key: &str) -> &'a Value {
    let nested = &v["error"][key];
    if nested.is_null() { &v[key] } else { nested }
}

/// Map a non-2xx gateway answer (CONTRACT §1 status codes) to what the UI shows.
pub fn map_error(status: u16, headers: &reqwest::header::HeaderMap, body: &str, claim_url_fallback: Option<&str>) -> PublikError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let kind = err_field(&v, "type").as_str().unwrap_or("").to_string();
    let retry = header_str(headers, "retry-after").and_then(|s| s.parse::<u64>().ok());
    match status {
        402 => {
            let claim_state = err_field(&v, "claim_state").as_str().unwrap_or("anonymous").to_string();
            let link = publik_link(err_field(&v, "top_up_url").as_str())
                .or_else(|| publik_link(err_field(&v, "claim_url").as_str()))
                .or_else(|| publik_link(claim_url_fallback))
                .or_else(|| Some(DASHBOARD_URL.to_string()));
            PublikError {
                status,
                kind: if kind.is_empty() { "insufficient_credit".into() } else { kind },
                message: err_field(&v, "message").as_str().filter(|m| !m.trim().is_empty()).unwrap_or("Not enough publik balance for this request.").to_string(),
                link_label: if claim_state == "claimed" { CTA_ADD_PLAN.into() } else { CTA_SIGN_IN.into() },
                link_url: link,
                retry_after_secs: None,
                reprovision: false,
            }
        }
        401 | 403 if kind == "key_revoked" => PublikError {
            status,
            kind,
            message: "This computer was removed from your publik account.".into(),
            link_label: "Open the publik dashboard".into(),
            link_url: Some(DASHBOARD_URL.into()),
            retry_after_secs: None,
            reprovision: err_field(&v, "reprovision").as_bool().unwrap_or(false),
        },
        401 => PublikError {
            status,
            kind: "invalid_api_key".into(),
            message: "publik API did not accept this computer's key. Turn publik API off and on again in Settings.".into(),
            link_label: String::new(),
            link_url: None,
            retry_after_secs: None,
            reprovision: false,
        },
        429 => PublikError {
            status,
            kind: if kind.is_empty() { "rate_limit_exceeded".into() } else { kind },
            message: err_field(&v, "message").as_str().unwrap_or("publik API asked us to slow down. Try again in a moment.").to_string(),
            link_label: String::new(),
            link_url: None,
            retry_after_secs: retry,
            reprovision: false,
        },
        503 => PublikError {
            status,
            kind: "gateway_unavailable".into(),
            message: "publik API is unavailable right now. Nothing was charged. Try again in a minute.".into(),
            link_label: String::new(),
            link_url: None,
            retry_after_secs: retry,
            reprovision: false,
        },
        _ => PublikError {
            status,
            kind: if kind.is_empty() { format!("http_{status}") } else { kind },
            message: err_field(&v, "message").as_str().map(String::from).unwrap_or_else(|| format!("publik API answered {status}.")),
            link_label: String::new(),
            link_url: None,
            retry_after_secs: None,
            reprovision: false,
        },
    }
}

/// Record the `x-publik-*` headers of a successful metered call.
pub fn note_response(h: &reqwest::header::HeaderMap) {
    let Some(balance) = header_i64(h, "x-publik-balance").or_else(|| header_i64(h, "x-publik-balance-micros")) else { return };
    with_live(|l| {
        l.balance_micros = Some(balance);
        l.last_charge_micros = header_i64(h, "x-publik-charge-micros").or(l.last_charge_micros);
        l.claim_state = header_str(h, "x-publik-claim-state").or(l.claim_state.take());
        l.week_used_micros = header_i64(h, "x-publik-week-used");
        l.week_budget_micros = header_i64(h, "x-publik-week-budget");
        l.model_served = header_str(h, "x-publik-model");
        l.exhausted = false;
        l.unreachable = false;
        l.disconnected = false;
        if l.notice.as_ref().map(|n| n.kind == "exhausted" || n.kind == "unreachable").unwrap_or(false) {
            l.notice = None;
        }
    });
}

/// Record a gateway failure so the status card can explain it.
pub fn note_error(e: &PublikError) {
    with_live(|l| {
        l.unreachable = e.status == 503 || e.status == 0;
        l.exhausted = e.status == 402;
        l.disconnected = e.kind == "key_revoked";
        if e.status == 402 {
            if let Some(a) = Some(e).filter(|_| true) {
                let _ = a;
            }
        }
        l.notice = Some(Notice {
            kind: match e.status { 402 => "exhausted", 503 | 0 => "unreachable", _ => "error" }.into(),
            message: e.message.clone(),
            link_label: e.link_label.clone(),
            link_url: e.link_url.clone().unwrap_or_default(),
        });
    });
}

pub fn note_transport_error(detail: &str) {
    with_live(|l| {
        l.unreachable = true;
        l.notice = Some(Notice {
            kind: "unreachable".into(),
            message: format!("publik API is unreachable right now ({detail}). Nothing was charged."),
            link_label: String::new(),
            link_url: String::new(),
        });
    });
}

fn client(secs: u64) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(secs)).user_agent(concat!("GSpot/", env!("CARGO_PKG_VERSION"))).build()?)
}

/// `POST /installs`: mint this computer's key. Only after the disclosure is accepted.
/// Replays (200 with `key: null`) are retried once with a fresh install id.
pub async fn provision(install_id: Option<String>) -> Result<Credential, PublikError> {
    let token = APP_TOKEN.filter(|t| !t.trim().is_empty()).ok_or_else(|| PublikError {
        status: 0,
        kind: "no_app_token".into(),
        message: "publik API is not available in this build (no app token was compiled in). Use your own key or a local model.".into(),
        link_label: String::new(),
        link_url: None,
        retry_after_secs: None,
        reprovision: false,
    })?;
    let base = resolve_base_url("");
    let mut id = install_id.filter(|s| !s.trim().is_empty()).unwrap_or_else(new_install_id);
    for attempt in 0..2 {
        let mut body = json!({
            "app_token": token,
            "app_slug": APP_SLUG,
            "app_version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "arch": arch_name(),
            "install_id": id,
            "disclosure_version": DISCLOSURE_VERSION,
            "dialects": ["chat_completions"],
        });
        if let Some(v) = os_version() {
            body["os_version"] = Value::String(v);
        }
        if let Some(d) = device_name() {
            body["device_name"] = Value::String(d);
        }
        let c = client(20).map_err(|e| transport(&e.to_string()))?;
        let res = c.post(format!("{base}/installs")).bearer_auth(token).header("Idempotency-Key", &id).json(&body).send().await.map_err(|e| transport(&e.to_string()))?;
        let status = res.status().as_u16();
        let headers = res.headers().clone();
        let text = res.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            let mut e = map_error(status, &headers, &text, None);
            if status == 401 || status == 403 {
                e.message = "publik could not set up this computer (the app token was not accepted). Use your own key or a local model.".into();
                e.link_url = None;
            }
            return Err(e);
        }
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let key = v["key"].as_str().unwrap_or("").trim().to_string();
        if key.is_empty() || status == 200 {
            // Replay of an install id we hold no credential for: mint a fresh one, once.
            if attempt == 0 {
                id = new_install_id();
                continue;
            }
            return Err(PublikError { status, kind: "replay".into(), message: "publik did not return a key for this computer. Try again.".into(), link_label: String::new(), link_url: None, retry_after_secs: None, reprovision: false });
        }
        let model = v["models"]["balanced"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(DEFAULT_MODEL).to_string();
        let cred = Credential {
            key,
            base_url: trim_url(v["base_url"].as_str().unwrap_or("")).unwrap_or(base.clone()),
            install_id: v["install_id"].as_str().unwrap_or(&id).to_string(),
            app_slug: APP_SLUG.into(),
            model,
            claim_url: publik_link(v["claim_url"].as_str()).unwrap_or_default(),
            add_credit_url: publik_link(v["add_credit_url"].as_str()).unwrap_or_default(),
            claim_state: v["claim_state"].as_str().unwrap_or("anonymous").to_string(),
            minted_at: chrono::Utc::now().to_rfc3339(),
        };
        let balance = v["balance_micros"].as_i64().or_else(|| v["starter_micros"].as_i64()).unwrap_or(0);
        with_live(|l| {
            l.balance_micros = Some(balance);
            l.claim_state = Some(cred.claim_state.clone());
            l.exhausted = false;
            l.unreachable = false;
            l.disconnected = false;
            l.notice = None;
        });
        return Ok(cred);
    }
    unreachable!()
}

fn transport(detail: &str) -> PublikError {
    PublikError { status: 0, kind: "transport".into(), message: format!("Could not reach publik ({detail}). Check your internet connection and try again."), link_label: String::new(), link_url: None, retry_after_secs: None, reprovision: false }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Wallet {
    pub balance_micros: i64,
    pub claim_state: String,
    pub claim_url: Option<String>,
    pub add_credit_url: Option<String>,
    pub top_up_url: Option<String>,
    pub week_used_micros: Option<i64>,
    pub week_budget_micros: Option<i64>,
}

/// `GET /wallet`: the balance line's source of truth when there is no fresh header.
pub async fn wallet(cred: &Credential) -> Result<Wallet, PublikError> {
    let c = client(10).map_err(|e| transport(&e.to_string()))?;
    let res = c.get(format!("{}/wallet", resolve_base_url(&cred.base_url))).bearer_auth(&cred.key).send().await.map_err(|e| transport(&e.to_string()))?;
    let status = res.status().as_u16();
    let headers = res.headers().clone();
    let text = res.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        let e = map_error(status, &headers, &text, Some(&cred.claim_url));
        note_error(&e);
        return Err(e);
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let balance = v["balance_micros"].as_i64().or_else(|| v["available_micros"].as_i64()).unwrap_or(0);
    let w = Wallet {
        balance_micros: balance,
        claim_state: v["claim_state"].as_str().unwrap_or("anonymous").to_string(),
        claim_url: publik_link(v["claim_url"].as_str()),
        add_credit_url: publik_link(v["add_credit_url"].as_str()),
        top_up_url: publik_link(v["top_up_url"].as_str()),
        week_used_micros: v["week"]["used_micros"].as_i64(),
        week_budget_micros: v["week"]["budget_micros"].as_i64(),
    };
    with_live(|l| {
        l.balance_micros = Some(w.balance_micros);
        l.claim_state = Some(w.claim_state.clone());
        l.week_used_micros = w.week_used_micros;
        l.week_budget_micros = w.week_budget_micros;
        l.unreachable = false;
        l.disconnected = false;
        if w.balance_micros > 0 {
            l.exhausted = false;
            if l.notice.as_ref().map(|n| n.kind == "exhausted").unwrap_or(false) {
                l.notice = None;
            }
        }
    });
    Ok(w)
}

/// One web search through publik API (`POST /search`): links and titles for `q`, at
/// publik's published price per search. A quoted `q` finds the exact phrase, which is
/// what the plagiarism check sends.
pub async fn search(cred: &Credential, q: &str, count: u8) -> Result<Vec<(String, String)>, PublikError> {
    let c = client(30).map_err(|e| transport(&e.to_string()))?;
    let res = c
        .post(format!("{}/search", resolve_base_url(&cred.base_url)))
        .bearer_auth(&cred.key)
        .json(&serde_json::json!({"q": q, "count": count}))
        .send()
        .await
        .map_err(|e| transport(&e.to_string()))?;
    let status = res.status().as_u16();
    let headers = res.headers().clone();
    let text = res.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        let e = map_error(status, &headers, &text, Some(&cred.claim_url));
        note_error(&e);
        return Err(e);
    }
    note_response(&headers);
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(v["results"]
        .as_array()
        .map(|a| a.iter().filter_map(|r| Some((r["url"].as_str()?.to_string(), r["title"].as_str().unwrap_or("").to_string()))).collect())
        .unwrap_or_default())
}

/// The credential the plagiarism check searches with: only when publik API is the
/// chosen provider. A user on their own model key keeps the keyless chain.
pub fn search_credential(settings: &crate::settings::Settings) -> Option<Credential> {
    if settings.llm.provider == crate::settings::Provider::Publik { read_credential() } else { None }
}

/// "$0.0004" / "$1.23".
pub fn fmt_usd(micros: i64) -> String {
    let d = micros as f64 / 1_000_000.0;
    if d.abs() < 0.01 && d != 0.0 {
        format!("${d:.4}")
    } else {
        format!("${d:.2}")
    }
}

/// The balance line: "$0.00 · link this computer for $0.05 of free use" while
/// anonymous, else the live balance.
pub fn balance_line(balance_micros: Option<i64>, claim_state: &str) -> String {
    let b = fmt_usd(balance_micros.unwrap_or(0));
    if claim_state == "claimed" {
        format!("{b} left")
    } else {
        format!("{b} · link this computer for $0.05 of free use")
    }
}

/// Everything the UI renders about publik API, from the credential + live state.
pub fn status(settings: &crate::settings::Settings) -> Value {
    let cred = read_credential();
    let l = live();
    let claim_state = l.claim_state.clone().or_else(|| cred.as_ref().map(|c| c.claim_state.clone())).filter(|s| !s.is_empty()).unwrap_or_else(|| "anonymous".into());
    let claimed = claim_state == "claimed";
    let claim_url = cred.as_ref().map(|c| c.claim_url.clone()).filter(|u| !u.is_empty());
    let add_credit_url = cred.as_ref().map(|c| c.add_credit_url.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| "https://publikhq.com/dashboard/api/add".into());
    let cta_url = if claimed { add_credit_url.clone() } else { claim_url.clone().unwrap_or_else(|| DASHBOARD_URL.into()) };
    let selected = settings.llm.provider == crate::settings::Provider::Publik;
    let state = match (&cred, l.disconnected, l.exhausted, l.unreachable) {
        (None, _, _, _) => "not_set_up",
        (_, true, _, _) => "disconnected",
        (_, _, true, _) => "needs_credit",
        (_, _, _, true) => "unreachable",
        _ => "ready",
    };
    json!({
        "available": can_provision() || cred.is_some(),
        "can_provision": can_provision(),
        "has_key": cred.is_some(),
        "selected": selected,
        "state": state,
        "claim_state": claim_state,
        "balance_micros": l.balance_micros,
        "balance_line": balance_line(l.balance_micros, &claim_state),
        "last_charge_label": l.last_charge_micros.map(|m| format!("last request {}", fmt_usd(m))),
        "week_label": match (l.week_used_micros, l.week_budget_micros) { (Some(u), Some(b)) if b > 0 => Some(format!("This week {} of {}", fmt_usd(u), fmt_usd(b))), _ => None },
        "model": cred.as_ref().map(|c| c.model.clone()).unwrap_or_else(|| DEFAULT_MODEL.into()),
        "model_served": l.model_served,
        "justification": WHY_IT_COSTS,
        "disclosure": {"cost": DISCLOSURE_COST, "data": DISCLOSURE_DATA, "terms_url": TERMS_URL},
        "cta": {"label": if claimed { CTA_ADD_PLAN } else { CTA_SIGN_IN }, "url": cta_url, "claimed": claimed},
        "later_label": CTA_LATER,
        "first_run_owed": settings.publik.first_run_owed && cred.is_some(),
        "dashboard_url": DASHBOARD_URL,
        "notice": l.notice,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_only_on_publikhq() {
        assert_eq!(publik_link(Some("https://publikhq.com/claim/AB12")), Some("https://publikhq.com/claim/AB12".into()));
        assert_eq!(publik_link(Some("https://evil.example/claim")), None);
        assert_eq!(publik_link(Some("http://publikhq.com/claim")), None);
        assert_eq!(publik_link(None), None);
    }

    #[test]
    fn usd_formatting() {
        assert_eq!(fmt_usd(0), "$0.00");
        assert_eq!(fmt_usd(50_000), "$0.05");
        assert_eq!(fmt_usd(400), "$0.0004");
        assert_eq!(fmt_usd(1_234_567), "$1.23");
    }

    #[test]
    fn balance_line_follows_claim_state() {
        assert_eq!(balance_line(None, "anonymous"), "$0.00 · link this computer for $0.05 of free use");
        assert_eq!(balance_line(Some(410_000), "claimed"), "$0.41 left");
    }

    #[test]
    fn install_ids_are_v4_uuids() {
        let id = new_install_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
        assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(id, new_install_id());
    }

    #[test]
    fn a_402_renders_the_message_and_exactly_one_link() {
        let body = r#"{"error":{"type":"insufficient_credit","message":"Not enough publik balance. Link this computer and pick a plan at the link below.","available_micros":0,"claim_state":"anonymous","top_up_url":"https://publikhq.com/claim/HK7F-2QWD","claim_url":"https://publikhq.com/claim/HK7F-2QWD"}}"#;
        let e = map_error(402, &reqwest::header::HeaderMap::new(), body, None);
        assert_eq!(e.kind, "insufficient_credit");
        assert!(e.message.starts_with("Not enough publik balance"));
        assert_eq!(e.link_url.as_deref(), Some("https://publikhq.com/claim/HK7F-2QWD"));
        assert_eq!(e.link_label, CTA_SIGN_IN);
        // claimed → the add-plan label
        let body2 = r#"{"error":{"type":"insufficient_credit","message":"m","claim_state":"claimed","top_up_url":"https://publikhq.com/dashboard/api/add"}}"#;
        let e2 = map_error(402, &reqwest::header::HeaderMap::new(), body2, None);
        assert_eq!(e2.link_label, CTA_ADD_PLAN);
        // a link off publikhq.com is dropped, the dashboard stands in
        let body3 = r#"{"error":{"type":"insufficient_credit","message":"m","top_up_url":"https://phish.example/x"}}"#;
        let e3 = map_error(402, &reqwest::header::HeaderMap::new(), body3, None);
        assert_eq!(e3.link_url.as_deref(), Some(DASHBOARD_URL));
    }

    #[test]
    fn revoked_and_unavailable_map_to_their_kinds() {
        let e = map_error(401, &reqwest::header::HeaderMap::new(), r#"{"error":{"type":"key_revoked","reprovision":true}}"#, None);
        assert_eq!(e.kind, "key_revoked");
        assert!(e.reprovision);
        let e = map_error(401, &reqwest::header::HeaderMap::new(), "{}", None);
        assert_eq!(e.kind, "invalid_api_key");
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("retry-after", "30".parse().unwrap());
        let e = map_error(503, &h, "", None);
        assert_eq!(e.retry_after_secs, Some(30));
    }

    #[test]
    fn credential_round_trips_and_defaults_model() {
        let c = Credential { key: "pk_live_abc".into(), base_url: "https://publikhq.com/api/v1/".into(), ..Default::default() };
        let j = serde_json::to_string(&c).unwrap();
        let back: Credential = serde_json::from_str(&j).unwrap();
        assert_eq!(back.key, "pk_live_abc");
        assert_eq!(resolve_base_url(&back.base_url), "https://publikhq.com/api/v1");
        assert_eq!(resolve_base_url(""), DEFAULT_BASE_URL);
    }

    #[test]
    fn headers_update_the_live_balance() {
        clear_live();
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-publik-balance", "41000".parse().unwrap());
        h.insert("x-publik-charge-micros", "120".parse().unwrap());
        h.insert("x-publik-claim-state", "claimed".parse().unwrap());
        note_response(&h);
        let l = live();
        assert_eq!(l.balance_micros, Some(41000));
        assert_eq!(l.last_charge_micros, Some(120));
        assert_eq!(l.claim_state.as_deref(), Some("claimed"));
        clear_live();
    }
}
