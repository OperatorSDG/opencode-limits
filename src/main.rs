use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use chrono::{DateTime, Utc};
use reqwest::Client;
use std::{env, sync::Arc};
use tokio::sync::RwLock;

struct AppState {
    client: Client,
    access_token: String,
    cache: RwLock<Option<CachedUsage>>,
}

#[derive(Debug)]
struct CachedUsage {
    data: ParsedUsage,
    fetched_at: DateTime<Utc>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct UpstreamUsage {
    user_id: String,
    account_id: String,
    email: String,
    plan_type: String,
    rate_limit: RateLimit,
    code_review_rate_limit: Option<RateLimit>,
    credits: Option<Credits>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct RateLimit {
    allowed: bool,
    limit_reached: bool,
    primary_window: Option<UsageWindow>,
    secondary_window: Option<UsageWindow>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct UsageWindow {
    used_percent: u32,
    limit_window_seconds: i64,
    reset_after_seconds: i64,
    reset_at: i64,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Credits {
    balance: String,
    has_credits: bool,
    unlimited: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
struct ParsedUsage {
    email: String,
    plan_type: String,
    session_used_percent: Option<u32>,
    session_reset_at: Option<i64>,
    session_reset_at_iso: Option<String>,
    weekly_used_percent: Option<u32>,
    weekly_reset_at: Option<i64>,
    weekly_reset_at_iso: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct UsageResponse {
    data: ParsedUsage,
    cache_age_seconds: i64,
    last_sync_unix: i64,
    last_sync_iso: String,
}

#[tokio::main]
async fn main() {
    let access_token =
        env::var("OPENAI_ACCESS_TOKEN").expect("set OPENAI_ACCESS_TOKEN in shell environment");

    let state = Arc::new(AppState {
        client: Client::new(),
        access_token,
        cache: RwLock::new(None),
    });

    let bg_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));

        if let Ok(data) = fetch_usage(&bg_state).await {
            let mut w = bg_state.cache.write().await;
            *w = Some(CachedUsage {
                data,
                fetched_at: Utc::now(),
            })
        }

        loop {
            interval.tick().await;
            match fetch_usage(&bg_state).await {
                Ok(data) => {
                    let mut w = bg_state.cache.write().await;
                    *w = Some(CachedUsage {
                        data,
                        fetched_at: Utc::now(),
                    })
                }
                Err(e) => eprintln!("refresh failed: {:?}", e),
            }
        }
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/usage", get(usage_raw))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("Failed to bind");

    println!("Listening to http://127.0.0.1:3000");
    axum::serve(listener, app).await.expect("Server Error!");
}

fn to_iso(ts: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(ts, 0).map(|dt| dt.to_rfc3339())
}

async fn health() -> &'static str {
    "ok\n"
}

async fn usage_raw(
    State(state): State<Arc<AppState>>,
) -> Result<Json<UsageResponse>, (StatusCode, String)> {
    let cache_guard = state.cache.read().await;
    if let Some(entry) = cache_guard.as_ref() {
        let age = Utc::now()
            .signed_duration_since(entry.fetched_at)
            .num_seconds();

        return Ok(Json(UsageResponse {
            data: entry.data.clone(),
            cache_age_seconds: age,
            last_sync_unix: entry.fetched_at.timestamp(),
            last_sync_iso: entry.fetched_at.to_rfc3339(),
        }));
    }

    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        "cache Warming up".to_string(),
    ))
}

async fn fetch_usage(state: &AppState) -> Result<ParsedUsage, (StatusCode, String)> {
    let resp = state
        .client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(&state.access_token)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Request failed: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp
            .text()
            .await
            .unwrap_or_else(|_| "<no upstream body>".to_string());
        return Err((status, format!("upstream error: {body}")));
    }

    let upstream: UpstreamUsage = resp.json().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("invalid upstream json: {e}"),
        )
    })?;

    let parsed = ParsedUsage {
        email: upstream.email,
        plan_type: upstream.plan_type,
        session_used_percent: upstream
            .rate_limit
            .primary_window
            .as_ref()
            .map(|w| w.used_percent),
        session_reset_at: upstream
            .rate_limit
            .primary_window
            .as_ref()
            .map(|w| w.reset_at),
        session_reset_at_iso: upstream
            .rate_limit
            .primary_window
            .as_ref()
            .and_then(|w| to_iso(w.reset_at)),
        weekly_used_percent: upstream
            .rate_limit
            .secondary_window
            .as_ref()
            .map(|w| w.used_percent),
        weekly_reset_at: upstream
            .rate_limit
            .secondary_window
            .as_ref()
            .map(|w| w.reset_at),
        weekly_reset_at_iso: upstream
            .rate_limit
            .secondary_window
            .as_ref()
            .and_then(|w| to_iso(w.reset_at)),
    };

    Ok(parsed)
}
