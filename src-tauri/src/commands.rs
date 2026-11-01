//! Tauri commands (and the plain functions behind them, shared with the debug probe).

use crate::engine::Alert;
use crate::features;
use crate::host::{HostDoc, Shared};
use crate::llm;
use crate::settings::{self, LlmConfig, Provider, Settings};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn doc_snapshot(shared: &Shared) -> Option<(HostDoc, crate::ax::AXElement)> {
    let st = shared.lock().unwrap();
    match (&st.doc, &st.element) {
        (Some(d), Some(e)) => Some((d.clone(), e.clone())),
        _ => None,
    }
}

pub fn accept_alert_inner(app: &AppHandle, shared: &Shared, id: &str, suggestion: usize) -> Result<crate::apply::ApplyResult, String> {
    let alert: Alert = {
        let st = shared.lock().unwrap();
        st.alerts.iter().find(|a| a.id == id).cloned().ok_or("That suggestion is no longer available.")?
    };
    let replacement = alert.suggestions.get(suggestion).cloned().ok_or("This suggestion has no automatic fix.")?;
    let (doc, el) = doc_snapshot(shared).ok_or("No text field is connected.")?;
    let res = crate::apply::replace_range(&el, doc.pid, alert.u16_start, alert.u16_end - alert.u16_start, &replacement, &doc.text).map_err(err)?;
    // Update the model immediately so the panel does not flicker.
    {
        let mut st = shared.lock().unwrap();
        if let Some(d) = st.doc.as_mut() {
            d.text = res.new_text.clone();
            d.version += 1;
        }
        st.last_text_hash = 0; // force the loop to re-read and re-lint
        st.alerts.retain(|a| a.id != id);
        st.spot_alerts.retain(|a| a.id != id);
        let alerts = st.alerts.clone();
        let doc = st.doc.clone();
        drop(st);
        let _ = app.emit("host://alerts", &alerts);
        if let Some(d) = doc {
            let _ = app.emit("host://document", &d);
        }
    }
    Ok(res)
}

pub fn dismiss_alert_inner(app: &AppHandle, shared: &Shared, id: &str) -> Result<(), String> {
    let mut st = shared.lock().unwrap();
    if let Some(a) = st.alerts.iter().find(|a| a.id == id).cloned() {
        st.settings.dismissed.insert(a.stable_hash);
        let _ = settings::save(&st.settings);
        st.alerts.retain(|x| x.stable_hash != a.stable_hash);
        st.spot_alerts.retain(|x| x.stable_hash != a.stable_hash);
    }
    let alerts = st.alerts.clone();
    drop(st);
    let _ = app.emit("host://alerts", &alerts);
    Ok(())
}

pub fn replace_range_inner(app: &AppHandle, shared: &Shared, start: usize, end: usize, text: &str) -> Result<crate::apply::ApplyResult, String> {
    let (doc, el) = doc_snapshot(shared).ok_or("No text field is connected.")?;
    let end = end.max(start);
    let res = crate::apply::replace_range(&el, doc.pid, start, end - start, text, &doc.text).map_err(err)?;
    {
        let mut st = shared.lock().unwrap();
        if let Some(d) = st.doc.as_mut() {
            d.text = res.new_text.clone();
            d.version += 1;
        }
        st.last_text_hash = 0;
        let doc = st.doc.clone();
        drop(st);
        if let Some(d) = doc {
            let _ = app.emit("host://document", &d);
        }
    }
    Ok(res)
}

fn dock_side(app: &AppHandle) -> String {
    app.state::<Shared>().lock().unwrap().settings.dock_side.clone()
}

/// Slide the panel out from the edge tab and switch it to `view`. `focus` also
/// makes it the key window (tray menu); the edge tab and G button leave the
/// host app's text field focused.
pub fn open_panel_inner(app: &AppHandle, view: &str, focus: bool) {
    let side = dock_side(app);
    crate::overlay::set_tucked(app, &side, false, true, focus);
    let _ = app.emit_to("panel", "panel://view", json!({"view": view}));
}

/// Slide the panel away behind the edge tab.
pub fn tuck_panel_inner(app: &AppHandle) {
    let side = dock_side(app);
    crate::overlay::set_tucked(app, &side, true, true, false);
}

/// Whole-document variant that still honours an explicit `text` argument (tests, chat chips).
fn whole_args(args: &Value) -> Value {
    json!({"whole": true, "text": args["text"].clone()})
}

fn resolve_text(shared: &Shared, args: &Value) -> Result<(String, String, bool), String> {
    // returns (text, app_name, used_selection)
    if let Some(t) = args["text"].as_str() {
        if !t.trim().is_empty() {
            let app_name = shared.lock().unwrap().doc.as_ref().map(|d| d.app_name.clone()).unwrap_or_default();
            return Ok((t.to_string(), app_name, false));
        }
    }
    let st = shared.lock().unwrap();
    let d = st.doc.as_ref().ok_or("Click into a text field first, then try again.")?;
    let whole = args["whole"].as_bool().unwrap_or(false);
    if !whole && !d.selected_text.trim().is_empty() {
        return Ok((d.selected_text.clone(), d.app_name.clone(), true));
    }
    if d.text.trim().is_empty() {
        return Err("The text field is empty.".into());
    }
    Ok((d.text.clone(), d.app_name.clone(), false))
}

/// The model configuration a feature actually calls: for publik API the key
/// and root come from the credential file (never from settings.json).
pub fn effective_llm(settings: &Settings) -> Result<LlmConfig, String> {
    match settings.llm.provider {
        Provider::Publik => {
            let cred = crate::publik::read_credential().ok_or("publik API is not set up on this computer yet. Open Settings → AI to turn it on, or paste your own key.")?;
            Ok(LlmConfig { provider: Provider::Publik, base_url: cred.base_url.clone(), api_key: cred.key.clone(), model: if settings.llm.model.trim().is_empty() { cred.model.clone() } else { settings.llm.model.clone() } })
        }
        Provider::Ollama => {
            if settings.llm.model.trim().is_empty() {
                return Err("Local AI is not set up yet. Open Settings → AI → Local AI (Ollama) and pick a model.".into());
            }
            Ok(settings.llm.clone())
        }
        _ => {
            if settings.llm.api_key.trim().is_empty() {
                return Err("No API key yet. Open Settings → AI and paste your key, or turn on publik API.".into());
            }
            Ok(settings.llm.clone())
        }
    }
}

/// Spot check the connected document (or `text` for tests): sentence-level
/// Clarity suggestions merged into the alert list and re-anchored to whatever
/// the document says once the model answers.
/// Take the spot-check slot for the connected document: `Ok(None)` while
/// another check runs (synchronous, so no lock is held across an await).
fn claim_spot(shared: &Shared) -> Result<Option<(String, String, features::spotcheck::Cache)>, String> {
    let mut st = shared.lock().unwrap();
    let (id, text) = st.doc.as_ref().map(|d| (d.id.clone(), d.text.clone())).ok_or("Click into a text field first, then try again.")?;
    if text.trim().is_empty() {
        return Err("The text field is empty.".into());
    }
    if st.spot_running {
        return Ok(None);
    }
    st.spot_running = true;
    Ok(Some((id, text, st.spot_cache.clone())))
}

async fn spot_check_inner(app: &AppHandle, shared: &Shared, cfg: Option<&LlmConfig>, args: &Value) -> Result<Value, String> {
    if let Some(t) = args["text"].as_str().filter(|t| !t.trim().is_empty()) {
        let mut cache = features::spotcheck::Cache::new();
        let started = std::time::Instant::now();
        let o = features::spotcheck::run(cfg, t, &mut cache).await.map_err(err)?;
        let mut v = serde_json::to_value(o).unwrap();
        v["ms"] = json!(started.elapsed().as_millis() as u64);
        return Ok(v);
    }
    // A manual check that lands while the host's automatic one is running
    // waits for it (the cache then answers for every unchanged sentence).
    let mut waited = 0u32;
    let (doc_id, text, mut cache) = loop {
        match claim_spot(shared)? {
            Some(v) => break v,
            None => {
                waited += 1;
                if waited > 450 {
                    return Ok(json!({"busy": true}));
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    };
    let started = std::time::Instant::now();
    let result = features::spotcheck::run(cfg, &text, &mut cache).await;
    let mut st = shared.lock().unwrap();
    st.spot_running = false;
    st.spot_cache = cache;
    let o = result.map_err(err)?;
    let same_doc = st.doc.as_ref().map(|d| d.id == doc_id).unwrap_or(false);
    if !same_doc {
        return Ok(json!({"stale": true, "checked": o.checked}));
    }
    let mut alerts = o.alerts.clone();
    if let Some(d) = st.doc.as_ref() {
        if d.text != text {
            features::spotcheck::re_anchor(&mut alerts, &d.text);
        }
    }
    st.spot_alerts = alerts;
    st.spot_doc_id = doc_id;
    st.alerts = st.visible_alerts();
    let list = st.alerts.clone();
    drop(st);
    let _ = app.emit("host://alerts", &list);
    Ok(json!({
        "sentences": o.sentences, "checked": o.checked, "from_cache": o.from_cache, "batches": o.batches,
        "skipped": o.skipped, "rejected": o.rejected, "rejections": o.rejections, "suggestions": o.alerts.len(), "ms": started.elapsed().as_millis() as u64,
    }))
}

/// Host-driven spot check: the host loop calls this once typing pauses, for
/// any panel view (or a tucked panel). Errors go to the log, not the user.
pub async fn spot_check_auto(app: AppHandle, shared: Shared, cfg: LlmConfig) {
    if let Err(e) = spot_check_inner(&app, &shared, Some(&cfg), &json!({})).await {
        log::warn!("auto spot check: {e}");
    }
    notify_fallback(&app);
}

/// Tell the panel once if the last answer came from the local fallback.
fn notify_fallback(app: &AppHandle) {
    if llm::take_fallback_notice() {
        let _ = app.emit("ai://notice", "Your cloud AI hit its limit or could not be reached, so this answer came from your local AI.");
    }
}

/// Ask the model for replacements for one flagged phrase and store them on the alert.
async fn word_options_inner(app: &AppHandle, shared: &Shared, cfg: &LlmConfig, id: &str) -> Result<Value, String> {
    let gone = "That suggestion is gone. Try again.";
    let (alert, text) = {
        let st = shared.lock().unwrap();
        (st.alerts.iter().find(|a| a.id == id).cloned(), st.doc.as_ref().map(|d| d.text.clone()))
    };
    let (alert, text) = alert.zip(text).ok_or(gone)?;
    let chars: Vec<char> = text.chars().collect();
    let is_end = |c: &char| matches!(c, '.' | '!' | '?' | '\n');
    let start = chars[..alert.start.min(chars.len())].iter().rposition(is_end).map(|i| i + 1).unwrap_or(0);
    let from = alert.end.min(chars.len());
    let end = chars[from..].iter().position(is_end).map(|i| from + i + 1).unwrap_or(chars.len());
    let sentence: String = chars[start..end.max(start)].iter().collect();
    let sentence: String = sentence.trim().chars().take(600).collect();
    let phrase = alert.original.trim();
    let issue = format!("{} - {}", alert.title, alert.message);
    let mut options = features::word_options(cfg, &sentence, phrase, &issue).await.map_err(err)?;
    if alert.original.ends_with(' ') {
        for o in options.iter_mut() {
            o.push(' ');
        }
    }
    let mut guard = shared.lock().unwrap();
    let st = &mut *guard;
    for a in st.all_alerts.iter_mut().chain(st.spot_alerts.iter_mut()).filter(|a| a.id == id) {
        a.suggestions = options.clone();
    }
    st.alerts = st.visible_alerts();
    let alerts = st.alerts.clone();
    drop(guard);
    let _ = app.emit("host://alerts", &alerts);
    Ok(json!({"options": options}))
}

pub async fn run_feature_inner(app: &AppHandle, shared: &Shared, name: &str, args: Value) -> Result<Value, String> {
    let settings: Settings = shared.lock().unwrap().settings.clone();
    let llm_cfg = effective_llm(&settings);
    let cfg = llm_cfg.clone().unwrap_or_else(|_| settings.llm.clone());
    let needs_llm = !matches!(name, "create_audience" | "rewriter_scan" | "plagiarism" | "score" | "spot_check" | "detect_ollama");
    if needs_llm {
        llm_cfg.clone()?;
    }
    let out = match name {
        // Without a model the spot check still runs its deterministic wordy-phrase rewrites.
        "spot_check" => spot_check_inner(app, shared, llm_cfg.as_ref().ok(), &args).await?,
        "paraphrase" => {
            let (text, _, sel) = resolve_text(shared, &args)?;
            let style = args["style"].as_str().unwrap_or("Professional");
            let mut v = features::paraphrase(&cfg, &text, style).await.map_err(err)?;
            v["used_selection"] = json!(sel);
            v
        }
        "humanize" => {
            let (text, _, sel) = resolve_text(shared, &args)?;
            let mut v = features::humanize(&cfg, &text, args["voice_name"].as_str().unwrap_or("The Everyday Voice"), args["voice_description"].as_str().unwrap_or("")).await.map_err(err)?;
            v["used_selection"] = json!(sel);
            v
        }
        "detect_ollama" => detect_ollama_inner().await,
        "ceo" => {
            let (text, _, sel) = resolve_text(shared, &args)?;
            let mut v = features::ceo::rewrite(&cfg, &text, args["level"].as_str().unwrap_or("casual")).await.map_err(err)?;
            v["used_selection"] = json!(sel);
            v
        }
        "create_voice" => {
            let sample = args["sample"].as_str().map(String::from).unwrap_or_else(|| resolve_text(shared, &whole_args(&args)).map(|t| t.0).unwrap_or_default());
            let v = features::create_voice(&cfg, &sample, args["name"].as_str().unwrap_or("")).await.map_err(err)?;
            let voice = settings::Voice { id: format!("v{}", chrono::Utc::now().timestamp_millis()), name: v["name"].as_str().unwrap_or("My voice").into(), description: v["description"].as_str().unwrap_or("").into(), custom: true };
            let mut st = shared.lock().unwrap();
            st.settings.custom_voices.push(voice.clone());
            let _ = settings::save(&st.settings);
            serde_json::to_value(voice).unwrap()
        }
        "reactions" => {
            let (text, _, _) = resolve_text(shared, &whole_args(&args))?;
            features::reader_reactions(&cfg, &text, args["audience"].as_str().unwrap_or("Online followers"), args["audience_desc"].as_str().unwrap_or("")).await.map_err(err)?
        }
        "create_audience" => {
            let a = settings::Audience { id: format!("a{}", chrono::Utc::now().timestamp_millis()), name: args["name"].as_str().unwrap_or("My audience").into(), description: args["description"].as_str().unwrap_or("").into(), custom: true };
            let mut st = shared.lock().unwrap();
            st.settings.custom_audiences.push(a.clone());
            let _ = settings::save(&st.settings);
            serde_json::to_value(a).unwrap()
        }
        "translate" => {
            let (text, _, sel) = resolve_text(shared, &args)?;
            let mut v = features::translate(&cfg, &text, args["target"].as_str().unwrap_or(&settings.target_language)).await.map_err(err)?;
            v["used_selection"] = json!(sel);
            v
        }
        "explain" => {
            let whole = args["whole"].as_bool().unwrap_or(false);
            let (text, _, sel) = resolve_text(shared, &args)?;
            let mut v = features::explain(&cfg, &text, whole || !sel).await.map_err(err)?;
            v["used_selection"] = json!(sel);
            v
        }
        "tone" => {
            let (text, _, _) = resolve_text(shared, &whole_args(&args))?;
            features::tone(&cfg, &text).await.map_err(err)?
        }
        "detector" => {
            let (text, _, _) = resolve_text(shared, &whole_args(&args))?;
            features::detector::detect(&cfg, &text).await.map_err(err)?
        }
        "rewriter_scan" => {
            let (text, _, _) = resolve_text(shared, &whole_args(&args))?;
            let found = crate::aiphrases::find(&text);
            let words = crate::score::word_count(&text);
            json!({"phrases": found, "words": words, "too_short": words < 8})
        }
        "rewrite_phrase" => features::rewrite_phrase(&cfg, args["sentence"].as_str().unwrap_or(""), args["phrase"].as_str().unwrap_or("")).await.map_err(err)?,
        "word_options" => word_options_inner(app, shared, &cfg, args["id"].as_str().unwrap_or("")).await?,
        "summarize" => {
            let (text, _, _) = resolve_text(shared, &whole_args(&args))?;
            json!({"summary": features::summarize(&cfg, &text).await.map_err(err)?})
        }
        "plagiarism" => {
            let (text, _, _) = resolve_text(shared, &whole_args(&args))?;
            features::plagiarism::check(&settings.search, crate::publik::search_credential(&settings).as_ref(), &text).await.map_err(err)?
        }
        "score" => {
            let st = shared.lock().unwrap();
            let d = st.doc.as_ref().ok_or("Click into a text field first.")?;
            serde_json::to_value(crate::score::compute(&d.text, &st.alerts)).unwrap()
        }
        "ping_llm" => {
            let r = llm::complete(&cfg, "Reply with the single word: pong", &[llm::msg("user", "ping")], false, 0.0).await.map_err(err)?;
            json!({"reply": r})
        }
        _ => return Err(format!("unknown feature {name}")),
    };
    notify_fallback(app);
    Ok(out)
}

#[tauri::command]
pub fn get_state(state: State<'_, Shared>) -> Value {
    let st = state.lock().unwrap();
    json!({
        "doc": st.doc,
        "alerts": st.alerts,
        "status": st.status,
        "settings": st.settings,
        "history": st.history,
        "score": st.doc.as_ref().map(|d| crate::score::compute(&d.text, &st.alerts)),
        "initials": settings::initials(&st.settings.user_name),
        "publik": crate::publik::status(&st.settings),
        "spot_running": st.spot_running,
    })
}

#[tauri::command]
pub fn accept_alert(app: AppHandle, state: State<'_, Shared>, id: String, suggestion: Option<usize>) -> Result<crate::apply::ApplyResult, String> {
    accept_alert_inner(&app, &state, &id, suggestion.unwrap_or(0))
}

#[tauri::command]
pub fn dismiss_alert(app: AppHandle, state: State<'_, Shared>, id: String) -> Result<(), String> {
    dismiss_alert_inner(&app, &state, &id)
}

#[tauri::command]
pub fn replace_range(app: AppHandle, state: State<'_, Shared>, start: usize, end: usize, text: String) -> Result<crate::apply::ApplyResult, String> {
    replace_range_inner(&app, &state, start, end, &text)
}

#[tauri::command]
pub async fn run_feature(app: AppHandle, state: State<'_, Shared>, name: String, args: Value) -> Result<Value, String> {
    let shared = state.inner().clone();
    run_feature_inner(&app, &shared, &name, args).await
}

#[tauri::command]
pub async fn chat_send(app: AppHandle, state: State<'_, Shared>, req_id: String, messages: Value) -> Result<String, String> {
    let (cfg, system) = {
        let st = state.lock().unwrap();
        let (doc_text, app_name, sel) = st.doc.as_ref().map(|d| (d.text.clone(), d.app_name.clone(), d.selected_text.clone())).unwrap_or_default();
        (effective_llm(&st.settings), features::chat_system(&st.settings.user_name, &app_name, &doc_text, &sel))
    };
    let cfg = match cfg {
        Ok(c) => c,
        Err(e) => {
            let _ = app.emit_to("panel", "llm://chunk", json!({"req_id": req_id, "delta": "", "done": true, "error": e}));
            return Err(e);
        }
    };
    let msgs = features::as_messages(&messages);
    let app2 = app.clone();
    let rid = req_id.clone();
    let result = llm::stream(&cfg, &system, &msgs, 0.5, |delta| {
        let _ = app2.emit_to("panel", "llm://chunk", json!({"req_id": rid, "delta": delta, "done": false}));
    })
    .await;
    notify_fallback(&app);
    match result {
        Ok(full) => {
            let _ = app.emit_to("panel", "llm://chunk", json!({"req_id": req_id, "delta": "", "done": true}));
            Ok(full)
        }
        Err(e) => {
            let _ = app.emit_to("panel", "llm://chunk", json!({"req_id": req_id, "delta": "", "done": true, "error": e.to_string()}));
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, Shared>) -> Settings {
    state.lock().unwrap().settings.clone()
}

#[tauri::command]
pub fn set_settings(app: AppHandle, state: State<'_, Shared>, settings: Settings) -> Result<(), String> {
    {
        let mut st = state.lock().unwrap();
        st.settings = settings.clone();
        llm::set_fallback_enabled(settings.ai_fallback_local);
        settings::save(&st.settings).map_err(err)?;
        // Re-apply the dismissed filter.
        st.alerts = st.visible_alerts();
    }
    crate::overlay::dock_panel(&app, &settings.dock_side);
    let _ = app.emit("settings://changed", &settings);
    Ok(())
}

#[tauri::command]
pub fn get_history(state: State<'_, Shared>) -> Vec<crate::history::Session> {
    state.lock().unwrap().history.clone()
}

#[tauri::command]
pub fn open_panel(app: AppHandle, view: Option<String>, focus: Option<bool>) {
    open_panel_inner(&app, view.as_deref().unwrap_or("review"), focus.unwrap_or(false));
}

#[tauri::command]
pub fn hide_panel(app: AppHandle) {
    tuck_panel_inner(&app);
}

#[tauri::command]
pub fn tuck_panel(app: AppHandle) {
    tuck_panel_inner(&app);
}

#[tauri::command]
pub fn permission_prompt() -> bool {
    crate::ax::prompt_trust()
}

#[tauri::command]
pub fn permission_status() -> bool {
    crate::ax::is_trusted()
}

#[tauri::command]
pub fn set_card_rects(state: State<'_, Shared>, rects: Vec<crate::ax::Rect>) {
    state.lock().unwrap().card_rects = rects;
}

#[tauri::command]
pub fn open_url(url: String) {
    // Web links only: never hand a file path or another scheme to the system opener.
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        log::warn!("open_url: refused a non-web url");
        return;
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", &url]).spawn();
}

#[tauri::command]
pub fn ui_ready(state: State<'_, Shared>, label: String) {
    let mut st = state.lock().unwrap();
    if !st.ui_ready.contains(&label) {
        st.ui_ready.push(label);
    }
}

#[tauri::command]
pub fn ui_log(state: State<'_, Shared>, message: String) {
    log::warn!("ui: {message}");
    let mut st = state.lock().unwrap();
    st.ui_log.push(message);
    if st.ui_log.len() > 50 {
        st.ui_log.remove(0);
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ---------------------------------------------------------------- publik API + key set-up

/// Persist a settings change made on the Rust side and tell every window.
fn save_settings(app: &AppHandle, shared: &Shared, f: impl FnOnce(&mut Settings)) -> Settings {
    let mut st = shared.lock().unwrap();
    f(&mut st.settings);
    llm::set_fallback_enabled(st.settings.ai_fallback_local);
    let _ = settings::save(&st.settings);
    st.alerts = st.visible_alerts();
    let s = st.settings.clone();
    drop(st);
    let _ = app.emit("settings://changed", &s);
    s
}

pub fn publik_status_inner(shared: &Shared) -> Value {
    let st = shared.lock().unwrap();
    crate::publik::status(&st.settings)
}

/// The user tapped "Turn on publik API" on the disclosure: mint this
/// computer's key, write the credential file, switch the model to publik and
/// owe the first-run card.
pub async fn publik_accept_inner(app: &AppHandle, shared: &Shared) -> Result<Value, String> {
    if let Some(existing) = crate::publik::read_credential() {
        // Never overwrite a key we already hold: just select it.
        save_settings(app, shared, |s| {
            s.llm = LlmConfig { provider: Provider::Publik, base_url: existing.base_url.clone(), api_key: String::new(), model: existing.model.clone() };
            s.publik.disclosure_version = crate::publik::DISCLOSURE_VERSION;
        });
        return Ok(publik_status_inner(shared));
    }
    let cred = crate::publik::provision(None).await.map_err(|e| e.message)?;
    crate::publik::write_credential(&cred).map_err(|e| format!("The key was minted but could not be saved: {e}"))?;
    save_settings(app, shared, |s| {
        s.llm = LlmConfig { provider: Provider::Publik, base_url: cred.base_url.clone(), api_key: String::new(), model: cred.model.clone() };
        s.publik.disclosure_version = crate::publik::DISCLOSURE_VERSION;
        s.publik.first_run_owed = true;
    });
    Ok(publik_status_inner(shared))
}

pub fn publik_dismiss_first_run_inner(app: &AppHandle, shared: &Shared) -> Value {
    save_settings(app, shared, |s| s.publik.first_run_owed = false);
    publik_status_inner(shared)
}

/// Forget this computer's key. The model setting returns to the publik default,
/// which then reports itself as not set up.
pub fn publik_forget_inner(app: &AppHandle, shared: &Shared) -> Value {
    crate::publik::forget_credential();
    save_settings(app, shared, |s| {
        s.publik.first_run_owed = false;
        if s.llm.provider == Provider::Publik {
            s.llm = LlmConfig::default();
        }
    });
    publik_status_inner(shared)
}

pub async fn publik_refresh_inner(shared: &Shared) -> Result<Value, String> {
    if let Some(cred) = crate::publik::read_credential() {
        let _ = crate::publik::wallet(&cred).await;
    }
    Ok(publik_status_inner(shared))
}

/// The account's chat model to use: the first preference it offers (exact, then by prefix);
/// otherwise an instruct/chat model, then any model that is not obviously non-chat.
fn pick_chat_model(prefs: &[&str], models: &[String]) -> Option<String> {
    const NOT_CHAT: &[&str] = &["embed", "whisper", "tts", "image", "guard", "rerank", "reward", "safety", "parse", "clip", "vlm", "vision", "video", "detector", "translate", "code", "coder", "deplot", "kosmos", "fuyu", "neva", "vila", "-vl-"];
    let chat = |m: &&String| !NOT_CHAT.iter().any(|x| m.contains(x));
    prefs
        .iter()
        .find(|p| models.iter().any(|m| m == *p))
        .map(|s| s.to_string())
        .or_else(|| prefs.iter().find_map(|p| models.iter().find(|m| m.starts_with(p)).cloned()))
        .or_else(|| models.iter().filter(chat).find(|m| m.contains("instruct") || m.ends_with("-it") || m.contains("chat")).cloned())
        .or_else(|| models.iter().find(chat).cloned())
}

/// Bring-your-own-key with one field: detect the provider from the key's
/// prefix, list the account's models, pick a sensible default, and ping it.
pub async fn probe_key_inner(key: &str) -> Result<Value, String> {
    let k = key.trim();
    if k.is_empty() {
        return Err("Paste a key first.".into());
    }
    let (provider, base, prefs): (Provider, &str, &[&str]) = if k.starts_with("sk-ant-") {
        (Provider::Anthropic, "https://api.anthropic.com", &["claude-sonnet-5-5", "claude-sonnet-4-5", "claude-opus-5-5", "claude-haiku-4-5", "claude-3-5-sonnet"])
    } else if k.starts_with("sk-or-") {
        (Provider::OpenAI, "https://openrouter.ai/api/v1", &["openai/gpt-5.6-terra", "anthropic/claude-sonnet-5-5", "openai/gpt-5.5", "openai/gpt-4o-mini", "openai/gpt-4o"])
    } else if k.starts_with("gsk_") {
        (Provider::OpenAI, "https://api.groq.com/openai", &["openai/gpt-oss-120b", "llama-3.3-70b-versatile", "openai/gpt-oss-20b", "llama-3.1-8b-instant"])
    } else if k.starts_with("AIza") {
        (Provider::OpenAI, "https://generativelanguage.googleapis.com/v1beta/openai", &["gemini-2.5-flash", "gemini-2.5-pro", "gemini-2.0-flash"])
    } else if k.starts_with("nvapi-") {
        (Provider::OpenAI, "https://integrate.api.nvidia.com", &["openai/gpt-oss-120b", "openai/gpt-oss-20b", "google/gemma-4-31b-it", "mistralai/mistral-large-2-instruct", "nvidia/nemotron-3-super-120b-a12b", "google/gemma-3-12b-it"])
    } else {
        (Provider::OpenAI, "https://api.openai.com", &["gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.5", "gpt-4.1-mini", "gpt-4o-mini", "gpt-4o"])
    };
    let mut cfg = LlmConfig { provider: provider.clone(), base_url: base.into(), api_key: k.into(), model: String::new() };
    let models = match provider {
        Provider::Anthropic => llm::anthropic_models(&cfg).await,
        _ => llm::openai_models(&cfg).await,
    }
    .map_err(|e| format!("The key was not accepted: {e}"))?;
    let pick = pick_chat_model(prefs, &models).ok_or("The key works, but no chat model is available on it.")?;
    cfg.model = pick;
    let reply = llm::complete(&cfg, "Reply with the single word: pong", &[llm::msg("user", "ping")], false, 0.0).await.map_err(|e| {
        // Some providers (NVIDIA) list models without a valid key, so a bad key only fails here.
        let t = e.to_string();
        if t.contains("401") || t.contains("403") {
            "The key was not accepted. Check that you copied all of it, then try again.".to_string()
        } else {
            format!("The key works but the model test failed: {t}")
        }
    })?;
    Ok(json!({
        "provider": serde_json::to_value(&provider).unwrap(),
        "base_url": base,
        "model": cfg.model,
        "models": models.len(),
        "reply": reply.trim(),
    }))
}

/// Is a local Ollama running, which models does it have, and which one fits best?
/// Backs the Local AI card in Settings and the debug probe (`/ollama`).
pub async fn detect_ollama_inner() -> Value {
    let base = crate::settings::OLLAMA_BASE_URL;
    let models = llm::ollama_models(base).await.unwrap_or_default();
    let pick = llm::pick_ollama_model(&models);
    json!({"running": !models.is_empty(), "base_url": base, "models": models, "pick": pick})
}

#[tauri::command]
pub fn publik_status(state: State<'_, Shared>) -> Value {
    publik_status_inner(&state)
}

#[tauri::command]
pub async fn publik_accept_disclosure(app: AppHandle, state: State<'_, Shared>) -> Result<Value, String> {
    let shared = state.inner().clone();
    publik_accept_inner(&app, &shared).await
}

#[tauri::command]
pub fn publik_dismiss_first_run(app: AppHandle, state: State<'_, Shared>) -> Value {
    publik_dismiss_first_run_inner(&app, &state)
}

#[tauri::command]
pub fn publik_forget(app: AppHandle, state: State<'_, Shared>) -> Value {
    publik_forget_inner(&app, &state)
}

#[tauri::command]
pub async fn publik_refresh(state: State<'_, Shared>) -> Result<Value, String> {
    let shared = state.inner().clone();
    publik_refresh_inner(&shared).await
}

#[tauri::command]
pub async fn probe_key(key: String) -> Result<Value, String> {
    probe_key_inner(&key).await
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_keys_pick_a_current_chat_model() {
        // A slice of NVIDIA's public /v1/models list (Nov 2026), alphabetical like the API.
        let nv: Vec<String> = ["01-ai/yi-large", "bigcode/starcoder2-15b", "google/gemma-4-31b-it", "nvidia/nv-embedqa-e5-v5", "openai/gpt-oss-20b", "nvidia/riva-translate-4b-instruct"].iter().map(|s| s.to_string()).collect();
        let prefs = ["openai/gpt-oss-120b", "openai/gpt-oss-20b", "google/gemma-4-31b-it"];
        assert_eq!(pick_chat_model(&prefs, &nv).as_deref(), Some("openai/gpt-oss-20b"));
        // With no preference on offer, an instruct model beats the first id in the list.
        assert_eq!(pick_chat_model(&["meta/llama-9"], &nv).as_deref(), Some("google/gemma-4-31b-it"));
        let only_odd: Vec<String> = vec!["nvidia/nv-embedqa-e5-v5".into(), "bigcode/starcoder2-15b".into()];
        assert_eq!(pick_chat_model(&[], &only_odd), None, "embeddings and code models are not chat models");
    }

    #[test]
    fn nvidia_endpoint_gets_v1() {
        assert_eq!(llm::openai_endpoint("https://integrate.api.nvidia.com", "chat/completions"), "https://integrate.api.nvidia.com/v1/chat/completions");
    }
}
