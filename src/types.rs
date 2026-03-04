#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ParsedUsage {
    pub email: String,
    pub plan_type: String,
    pub session_used_percent: Option<u32>,
    pub session_reset_at: Option<i64>,
    pub session_reset_at_iso: Option<String>,
    pub weekly_used_percent: Option<u32>,
    pub weekly_reset_at: Option<i64>,
    pub weekly_reset_at_iso: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct UsageResponse {
    pub data: ParsedUsage,
    pub cache_age_seconds: i64,
    pub last_sync_unix: i64,
    pub last_sync_iso: String,
}
