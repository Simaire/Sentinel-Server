//! Présence des modos en ligne, gardée en mémoire : aucune écriture en base à chaque
//! actualisation, et rien à relire pour les autres applis.

use crate::auth::ActiveUser;
use crate::error::{ApiError, ApiResult};
use crate::state::Shared;
use crate::util::now_unix;
use axum::extract::State;
use axum::Json;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// Un modo est « en ligne » s'il a donné signe de vie depuis moins longtemps.
pub const ONLINE_SECS: i64 = 180;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PresenceInfo {
    pub location: Option<String>,
    pub world_name: Option<String>,
    #[serde(default)]
    pub user_count: u32,
    #[serde(default)]
    pub danger: u32,
    #[serde(default)]
    pub caution: u32,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModeratorOnline {
    pub user_id: String,
    pub display_name: Option<String>,
    pub location: Option<String>,
    pub world_name: Option<String>,
    pub user_count: u32,
    pub danger: u32,
    pub caution: u32,
    pub version: Option<String>,
    pub last_seen_at: i64,
}

#[derive(Default)]
pub struct Presence {
    online: DashMap<String, ModeratorOnline>,
}

fn clip(value: Option<String>, max: usize) -> Option<String> {
    value.map(|v| v.chars().take(max).collect())
}

impl Presence {
    pub fn update(&self, user: &ActiveUser, info: PresenceInfo) {
        self.online.insert(
            user.id.clone(),
            ModeratorOnline {
                user_id: user.id.clone(),
                display_name: Some(user.display_name.clone()),
                location: clip(info.location, 300),
                world_name: clip(info.world_name, 200),
                user_count: info.user_count.min(1_000),
                danger: info.danger.min(1_000),
                caution: info.caution.min(1_000),
                version: clip(info.version, 32),
                last_seen_at: now_unix(),
            },
        );
    }

    pub fn online(&self) -> Vec<ModeratorOnline> {
        let since = now_unix() - ONLINE_SECS;
        let mut list: Vec<ModeratorOnline> = self
            .online
            .iter()
            .filter(|m| m.last_seen_at >= since)
            .map(|m| m.clone())
            .collect();
        list.sort_by_key(|m| std::cmp::Reverse(m.last_seen_at));
        list
    }

    pub fn remove(&self, user_id: &str) {
        self.online.remove(user_id);
    }

    /// Oublie les modos partis depuis longtemps.
    pub fn sweep(&self) {
        let since = now_unix() - ONLINE_SECS * 10;
        self.online.retain(|_, m| m.last_seen_at >= since);
    }
}

pub async fn post(State(state): State<Shared>, user: ActiveUser, Json(info): Json<PresenceInfo>) -> ApiResult<Json<Vec<ModeratorOnline>>> {
    if !state.limits.presence.allow(&user.id) {
        return Err(ApiError::TooManyRequests);
    }
    state.presence.update(&user, info);
    Ok(Json(state.presence.online()))
}

pub async fn list(State(state): State<Shared>, _user: ActiveUser) -> Json<Vec<ModeratorOnline>> {
    Json(state.presence.online())
}
