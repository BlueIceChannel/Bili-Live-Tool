use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoginState {
    LoggedIn,
    NeedQrCode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrCodeData {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebQrInfo {
    pub url: String,
    pub qrcode_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoomInfo {
    pub room_id: u64,
    pub title: String,
    pub cover_url: String,
    pub area_id: u64,
    pub area_name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenInfo {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub expires: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthData {
    pub token: TokenInfo,
    pub cookies: Vec<Cookie>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LiveRoomBrief {
    pub room_status: i32,
    pub live_status: i32,
    pub title: String,
    pub cover: String,
    pub room_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserInfo {
    pub mid: u64,
    pub name: String,
    pub face: String,
    pub live_room: LiveRoomBrief,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AreaChild {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AreaParent {
    pub id: i64,
    pub name: String,
    pub children: Vec<AreaChild>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditInfo {
    pub audit_title_status: i32,
    pub audit_title_reason: String,
} 