//! 与 B 站交互的 HTTP 客户端，占位实现。

use anyhow::Result;
use domain::{LoginState, QrCodeData, RoomInfo, TokenInfo, Cookie as CookieInfo, AuthData, AreaParent, AreaChild, AuditInfo};
use reqwest::Client;
use md5::{Digest, Md5};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use reqwest::cookie::Jar;
use rand::{seq::SliceRandom, thread_rng};
use reqwest::header::USER_AGENT;
use rsa::{pkcs8::DecodePublicKey, Oaep};
use sha2::Sha256;
use hex;
use regex::Regex;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use chrono;

const APP_KEY: &str = "4409e2ce8ffd12b8"; // 云视听小电视
const APP_SEC: &str = "59b43e04ad6965f34319062b478f83dd";

const USER_AGENTS: &[&str] = &[
    // 常见浏览器 UA
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/118.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 13_4_1) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.5 Safari/605.1.15",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148",
    "Mozilla/5.0 (Linux; Android 12; Pixel 6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/118.0.0.0 Mobile Safari/537.36",
    // B 站 TV / Pad 客户端 UA 示例
    "Mozilla/5.0 BiliTV/1110500 (Linux; Android 11) bilibili-tv;free",
];

const PUB_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDLgd2OAkcGVtoE3ThUREbio0Eg\nUc/prcajMKXvkCKFCWhJYJcLkcM2DKKcSeFpD/j6Boy538YXnR6VhcuUJOhH2x71\nnzPjfdTcqMz7djHum0qSZA0AyCBDABUqCrfNgCiJ00Ra7GmRj+YCK1NJEuewlb40\nJNrRuoEUXpabUzGB8QIDAQAB\n-----END PUBLIC KEY-----";

const MIXIN_KEY_TAB: [u8; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49,
    33, 9, 42, 19, 29, 28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40,
    61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25, 54, 21, 56, 59, 6, 63, 57, 62, 11,
    36, 20, 34, 44, 52,
];

fn calc_sign(params: &BTreeMap<&str, String>) -> String {
    let mut query = String::new();
    for (i, (k, v)) in params.iter().enumerate() {
        if i > 0 {
            query.push('&');
        }
        query.push_str(k);
        query.push('=');
        query.push_str(v);
    }
    query.push_str(APP_SEC);
    let digest = Md5::digest(query.as_bytes());
    format!("{:x}", digest)
}

fn current_ts() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}

pub struct BiliClient {
    client: Client,
    jar: Arc<Jar>,
}

impl BiliClient {
    fn auth_file_path() -> Option<PathBuf> {
        ProjectDirs::from("com", "Bili", "LiveTool").map(|proj| proj.config_dir().join("auth.json"))
    }

    fn load_auth() -> Option<AuthData> {
        let path = Self::auth_file_path()?;
        let content = fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn save_auth(auth: &AuthData) -> anyhow::Result<()> {
        if let Some(path) = Self::auth_file_path() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let data = serde_json::to_string_pretty(auth)?;
            fs::write(path, data)?;
        }
        Ok(())
    }

    /// 创建客户端实例，稍后可注入 Cookie / Token
    pub fn new() -> Self {
        let jar = Arc::new(Jar::default());
        if let Some(auth) = Self::load_auth() {
            for c in &auth.cookies {
                let cookie_str = format!("{}={}", c.name, c.value);
                let url = format!("https://{}", c.domain).parse().unwrap();
                jar.add_cookie_str(&cookie_str, &url);
            }
        }
        let client = Client::builder()
            .cookie_provider(jar.clone())
            .user_agent("BiliLiveTool/0.1")
            .build()
            .expect("reqwest client build failed");
        Self { client, jar }
    }

    fn random_ua() -> &'static str {
        USER_AGENTS.choose(&mut thread_rng()).copied().unwrap_or(USER_AGENTS[0])
    }

    async fn post_form_retry(&self, url: &str, params: &BTreeMap<&str, String>) -> anyhow::Result<serde_json::Value> {
        let mut attempts = 0;
        let mut last_err: anyhow::Error = anyhow::anyhow!("unknown");
        while attempts < 3 {
            let ua = Self::random_ua();
            let resp = self
                .client
                .post(url)
                .header(USER_AGENT, ua)
                .form(params)
                .send()
                .await;
            match resp {
                Ok(r) => {
                    let status = r.status();
                    let json_val: serde_json::Value = r.json().await.unwrap_or_default();
                    // 如果 HTTP 被拦截（412）或 code == -412，尝试更换 UA
                    if status.as_u16() == 412 || json_val["code"].as_i64().unwrap_or(0) == -412 {
                        attempts += 1;
                        continue;
                    }
                    return Ok(json_val);
                }
                Err(e) => {
                    last_err = e.into();
                    attempts += 1;
                }
            }
        }
        Err(last_err)
    }

    /// 检查当前登录状态
    pub async fn check_login_state(&self) -> Result<LoginState> {
        // TODO: 实现真正的逻辑
        Ok(LoginState::NeedQrCode)
    }

    /// 获取登录二维码
    pub async fn fetch_qr_code(&self) -> Result<QrCodeData> {
        let mut params: BTreeMap<&str, String> = BTreeMap::new();
        params.insert("appkey", APP_KEY.to_string());
        params.insert("local_id", "0".into());
        let ts = current_ts();
        params.insert("ts", ts.clone());
        let sign = calc_sign(&params);
        params.insert("sign", sign);

        let resp = self
            .post_form_retry(
                "https://passport.snm0516.aisee.tv/x/passport-tv-login/qrcode/auth_code",
                &params,
            )
            .await?;

        if resp["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("获取二维码失败: {}", resp["message"].as_str().unwrap_or(""));
        }
        let data = &resp["data"];
        Ok(QrCodeData {
            url: data["url"].as_str().unwrap_or("").to_string(),
        })
    }

    /// 轮询二维码是否扫描完成
    pub async fn poll_qr_login(&self, qr: &QrCodeData) -> Result<LoginState> {
        // 从二维码 url 中提取 auth_code
        let auth_code = qr
            .url
            .split("auth_code=")
            .nth(1)
            .unwrap_or("")
            .to_string();

        let mut params: BTreeMap<&str, String> = BTreeMap::new();
        params.insert("appkey", APP_KEY.to_string());
        params.insert("auth_code", auth_code);
        params.insert("local_id", "0".into());
        let ts = current_ts();
        params.insert("ts", ts.clone());
        let sign = calc_sign(&params);
        params.insert("sign", sign);

        let resp = self
            .post_form_retry(
                "https://passport.snm0516.aisee.tv/x/passport-tv-login/qrcode/poll",
                &params,
            )
            .await?;

        let code = resp["code"].as_i64().unwrap_or(-1);
        match code {
            0 => {
                // 解析 token 和 cookie
                let data = &resp["data"];
                let token_info = TokenInfo {
                    access_token: data["access_token"].as_str().unwrap_or("").to_string(),
                    refresh_token: data["refresh_token"].as_str().unwrap_or("").to_string(),
                    expires_in: data["expires_in"].as_i64().unwrap_or(0),
                };
                let mut cookies_vec = Vec::new();
                if let Some(cookie_arr) = data["cookie_info"]["cookies"].as_array() {
                    for c in cookie_arr {
                        let cookie = CookieInfo {
                            name: c["name"].as_str().unwrap_or("").to_string(),
                            value: c["value"].as_str().unwrap_or("").to_string(),
                            domain: ".bilibili.com".to_string(),
                            expires: c["expires"].as_i64().unwrap_or(0),
                        };
                        let cookie_str = format!("{}={}", cookie.name, cookie.value);
                        let url = format!("https://{}", cookie.domain).parse().unwrap();
                        self.jar.add_cookie_str(&cookie_str, &url);
                        cookies_vec.push(cookie);
                    }
                }
                let auth_data = AuthData { token: token_info, cookies: cookies_vec };
                Self::save_auth(&auth_data)?;
                Ok(LoginState::LoggedIn)
            }
            86038 => Ok(LoginState::NeedQrCode), // 二维码失效，需要重新获取
            86039 | 86090 => Ok(LoginState::NeedQrCode), // 尚未登录或未确认
            _ => anyhow::bail!("二维码登录失败: {}", resp["message"].as_str().unwrap_or("")),
        }
    }

    /// 获取直播间信息
    pub async fn get_room_info(&self) -> Result<RoomInfo> {
        // TODO: 实现真正的逻辑
        Ok(RoomInfo::default())
    }

    /// 更新直播间信息：支持修改标题与分区。返回审核信息（若有）。
    pub async fn update_room_info(&self, room_id: i64, title: Option<&str>, area_id: Option<i64>) -> anyhow::Result<Option<AuditInfo>> {
        let csrf = self.get_cookie_value("bili_jct").ok_or_else(|| anyhow::anyhow!("缺少 csrf cookie"))?;
        let mut params: BTreeMap<&str, String> = BTreeMap::new();
        params.insert("csrf", csrf.clone());
        params.insert("csrf_token", csrf.clone());
        params.insert("room_id", room_id.to_string());
        if let Some(t) = title {
            params.insert("title", t.to_string());
        }
        if let Some(a) = area_id {
            params.insert("area_id", a.to_string());
        }
        let resp = self.post_form_retry("https://api.live.bilibili.com/room/v1/Room/update", &params).await?;
        let code = resp["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            anyhow::bail!("更新失败: {}", resp["message"].as_str().unwrap_or(""));
        }
        let audit = &resp["data"]["audit_info"];
        if audit.is_object() {
            Ok(Some(AuditInfo {
                audit_title_status: audit["audit_title_status"].as_i64().unwrap_or(0) as i32,
                audit_title_reason: audit["audit_title_reason"].as_str().unwrap_or("").to_string(),
            }))
        } else {
            Ok(None)
        }
    }

    /// 开始直播，返回 (addr, code)
    pub async fn start_live(&self, room_id: i64, area_id: i64) -> anyhow::Result<(String, String)> {
        let csrf = self.get_cookie_value("bili_jct").ok_or_else(|| anyhow::anyhow!("缺少 csrf cookie"))?;
        let mut params: BTreeMap<&str, String> = BTreeMap::new();
        params.insert("room_id", room_id.to_string());
        params.insert("area_v2", area_id.to_string());
        params.insert("platform", "pc_link".to_string());
        params.insert("csrf", csrf.clone());

        let resp = self.post_form_retry("https://api.live.bilibili.com/room/v1/Room/startLive", &params).await?;
        if resp["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("开播失败: {}", resp["message"].as_str().unwrap_or(""));
        }
        let rtmp = &resp["data"]["rtmp"];
        let addr = rtmp["addr"].as_str().unwrap_or("").to_string();
        let code = rtmp["code"].as_str().unwrap_or("").to_string();
        Ok((addr, code))
    }

    /// 停止直播
    pub async fn stop_live(&self, room_id: i64) -> anyhow::Result<()> {
        let csrf = self.get_cookie_value("bili_jct").ok_or_else(|| anyhow::anyhow!("缺少 csrf cookie"))?;
        let mut params: BTreeMap<&str, String> = BTreeMap::new();
        params.insert("room_id", room_id.to_string());
        params.insert("platform", "pc_link".to_string());
        params.insert("csrf", csrf.clone());
        let resp = self.post_form_retry("https://api.live.bilibili.com/room/v1/Room/stopLive", &params).await?;
        if resp["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("关播失败: {}", resp["message"].as_str().unwrap_or(""));
        }
        Ok(())
    }

    fn get_cookie_value(&self, name: &str) -> Option<String> {
        let url = "https://bilibili.com".parse().ok()?;
        let cookies = self.jar.cookies(&url)?;
        let cookie_str = cookies.to_str().ok()?;
        for part in cookie_str.split(';') {
            let mut kv = part.trim().splitn(2, '=');
            let k = kv.next()?;
            let v = kv.next()?;
            if k == name {
                return Some(v.to_string());
            }
        }
        None
    }

    fn build_cookie_list(&self) -> Vec<CookieInfo> {
        // 仅简单解析常用 cookie 并存储
        let url = "https://bilibili.com".parse().unwrap();
        if let Some(cookies) = self.jar.cookies(&url) {
            if let Ok(s) = cookies.to_str() {
                return s.split(';')
                    .filter_map(|item| {
                        let item = item.trim();
                        let mut kv = item.splitn(2, '=');
                        let name = kv.next()?;
                        let value = kv.next()?;
                        Some(CookieInfo {
                            name: name.to_string(),
                            value: value.to_string(),
                            domain: ".bilibili.com".to_string(),
                            expires: 0,
                        })
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    fn generate_correspond_path(ts: i64) -> anyhow::Result<String> {
        let public_key = rsa::RsaPublicKey::from_public_key_pem(PUB_KEY_PEM)?;
        let plaintext = format!("refresh_{}", ts);
        let padding = Oaep::new::<Sha256>();
        let mut rng = rand::thread_rng();
        let encrypted = public_key.encrypt(&mut rng, padding, plaintext.as_bytes())?;
        Ok(hex::encode(encrypted))
    }

    pub async fn refresh_cookies_if_needed(&self) -> anyhow::Result<()> {
        // 1. 获取 csrf
        let csrf = match self.get_cookie_value("bili_jct") {
            Some(c) => c,
            None => return Ok(()), // 未登录，无需刷新
        };

        // 2. 检查是否需要刷新
        let mut query = vec![("csrf", csrf.as_str())];
        let check_url = "https://passport.bilibili.com/x/passport-login/web/cookie/info";
        let resp_json: serde_json::Value = self
            .client
            .get(check_url)
            .header(USER_AGENT, Self::random_ua())
            .query(&query)
            .send()
            .await?
            .json()
            .await?;
        if resp_json["code"].as_i64().unwrap_or(-1) != 0 {
            return Ok(()); // 无法检查，忽略
        }
        let data = &resp_json["data"];
        let need_refresh = data["refresh"].as_bool().unwrap_or(false);
        if !need_refresh {
            return Ok(());
        }
        let timestamp = data["timestamp"].as_i64().unwrap_or_else(|| {
            let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            dur.as_millis() as i64
        });

        // 3. 生成 correspondPath
        let correspond_path = Self::generate_correspond_path(timestamp)?;

        // 4. 获取 refresh_csrf
        let correspond_url = format!("https://www.bilibili.com/correspond/1/{}", correspond_path);
        let html_text = self
            .client
            .get(&correspond_url)
            .header(USER_AGENT, Self::random_ua())
            .send()
            .await?
            .text()
            .await?;
        let re = Regex::new(r#"<div id=['\"]1-name['\"]>([0-9a-f]{32})</div>"#).unwrap();
        let refresh_csrf = match re.captures(&html_text) {
            Some(caps) => caps.get(1).unwrap().as_str().to_string(),
            None => anyhow::bail!("无法解析 refresh_csrf"),
        };

        // 5. 准备刷新 cookie
        let auth_opt = Self::load_auth();
        let refresh_token_old = match &auth_opt {
            Some(a) => a.token.refresh_token.clone(),
            None => String::new(),
        };
        if refresh_token_old.is_empty() {
            anyhow::bail!("缺少 refresh_token，无法刷新 cookie");
        }

        let mut form: BTreeMap<&str, String> = BTreeMap::new();
        form.insert("csrf", csrf.clone());
        form.insert("refresh_csrf", refresh_csrf);
        form.insert("source", "main_web".into());
        form.insert("refresh_token", refresh_token_old.clone());

        let refresh_resp: serde_json::Value = self
            .client
            .post("https://passport.bilibili.com/x/passport-login/web/cookie/refresh")
            .header(USER_AGENT, Self::random_ua())
            .form(&form)
            .send()
            .await?
            .json()
            .await?;
        if refresh_resp["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("刷新 cookie 失败: {}", refresh_resp["message"].as_str().unwrap_or(""));
        }
        let new_refresh_token = refresh_resp["data"]["refresh_token"].as_str().unwrap_or("").to_string();

        // 6. 确认更新，让旧 refresh_token 失效
        let csrf_new = match self.get_cookie_value("bili_jct") {
            Some(c) => c,
            None => csrf.clone(),
        };
        let mut confirm_form: BTreeMap<&str, String> = BTreeMap::new();
        confirm_form.insert("csrf", csrf_new);
        confirm_form.insert("refresh_token", refresh_token_old.clone());
        let _ = self
            .client
            .post("https://passport.bilibili.com/x/passport-login/web/confirm/refresh")
            .header(USER_AGENT, Self::random_ua())
            .form(&confirm_form)
            .send()
            .await;

        // 7. 保存最新 auth 数据
        let (old_access, old_expire) = match &auth_opt {
            Some(a) => (a.token.access_token.clone(), a.token.expires_in),
            None => (String::new(), 0),
        };
        let token_info = TokenInfo {
            access_token: old_access,
            refresh_token: new_refresh_token,
            expires_in: old_expire,
        };
        let cookies_vec = self.build_cookie_list();
        let auth_data = AuthData { token: token_info, cookies: cookies_vec };
        let _ = Self::save_auth(&auth_data);

        Ok(())
    }

    async fn get_wbi_keys(&self) -> anyhow::Result<(String, String)> {
        let resp: serde_json::Value = self
            .client
            .get("https://api.bilibili.com/x/web-interface/nav")
            .header(USER_AGENT, Self::random_ua())
            .send()
            .await?
            .json()
            .await?;
        let img_url = resp["data"]["wbi_img"]["img_url"].as_str().unwrap_or("");
        let sub_url = resp["data"]["wbi_img"]["sub_url"].as_str().unwrap_or("");
        let img_key = img_url
            .split('/')
            .last()
            .unwrap_or("")
            .split('.')
            .next()
            .unwrap_or("")
            .to_string();
        let sub_key = sub_url
            .split('/')
            .last()
            .unwrap_or("")
            .split('.')
            .next()
            .unwrap_or("")
            .to_string();
        Ok((img_key, sub_key))
    }

    fn calc_mixin_key(img_key: &str, sub_key: &str) -> String {
        let raw = format!("{}{}", img_key, sub_key);
        let mut mixed: Vec<u8> = MIXIN_KEY_TAB
            .iter()
            .filter_map(|&i| raw.as_bytes().get(i as usize).copied())
            .collect();
        mixed.truncate(32);
        unsafe { String::from_utf8_unchecked(mixed) }
    }

    fn encode_params(params: &BTreeMap<&str, String>) -> String {
        let mut encoded_pairs: Vec<(String, String)> = params
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    utf8_percent_encode(v, NON_ALPHANUMERIC).to_string(),
                )
            })
            .collect();
        // sort by key ascending
        encoded_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        encoded_pairs
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&")
    }

    async fn signed_wbi_query(&self, mut params: BTreeMap<&str, String>) -> anyhow::Result<BTreeMap<String, String>> {
        let (img_key, sub_key) = self.get_wbi_keys().await?;
        let mixin_key = Self::calc_mixin_key(&img_key, &sub_key);
        let wts = chrono::Utc::now().timestamp();
        params.insert("wts", wts.to_string());
        let query_sorted = Self::encode_params(&params);
        let sign_str = format!("{}{}", query_sorted, mixin_key);
        let w_rid = format!("{:x}", Md5::digest(sign_str.as_bytes()));
        let mut out = BTreeMap::new();
        for (k, v) in params {
            out.insert(k.to_string(), v);
        }
        out.insert("w_rid".to_string(), w_rid);
        Ok(out)
    }

    pub async fn get_user_info(&self, mid: u64) -> anyhow::Result<UserInfo> {
        let mut params: BTreeMap<&str, String> = BTreeMap::new();
        params.insert("mid", mid.to_string());
        let signed = self.signed_wbi_query(params).await?;
        let resp: serde_json::Value = self
            .client
            .get("https://api.bilibili.com/x/space/wbi/acc/info")
            .header(USER_AGENT, Self::random_ua())
            .query(&signed)
            .send()
            .await?
            .json()
            .await?;
        if resp["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("获取用户信息失败: {}", resp["message"].as_str().unwrap_or(""));
        }
        let d = &resp["data"];
        let live = &d["live_room"];
        let user = UserInfo {
            mid: d["mid"].as_u64().unwrap_or(0),
            name: d["name"].as_str().unwrap_or("").to_string(),
            face: d["face"].as_str().unwrap_or("").to_string(),
            live_room: LiveRoomBrief {
                room_status: live["roomStatus"].as_i64().unwrap_or(0) as i32,
                live_status: live["liveStatus"].as_i64().unwrap_or(0) as i32,
                title: live["title"].as_str().unwrap_or("").to_string(),
                cover: live["cover"].as_str().unwrap_or("").to_string(),
                room_id: live["roomid"].as_i64().unwrap_or(0),
            },
        };
        Ok(user)
    }

    pub async fn get_area_list(&self) -> anyhow::Result<Vec<AreaParent>> {
        let resp: serde_json::Value = self
            .client
            .get("https://api.live.bilibili.com/room/v1/Area/getList")
            .header(USER_AGENT, Self::random_ua())
            .send()
            .await?
            .json()
            .await?;
        if resp["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("获取分区失败: {}", resp["message"].as_str().unwrap_or(""));
        }
        let mut parents = Vec::new();
        if let Some(arr) = resp["data"].as_array() {
            for p in arr {
                let mut children = Vec::new();
                if let Some(list) = p["list"].as_array() {
                    for c in list {
                        children.push(AreaChild {
                            id: c["id"].as_str().unwrap_or("0").parse().unwrap_or(0),
                            name: c["name"].as_str().unwrap_or("").to_string(),
                        });
                    }
                }
                parents.push(AreaParent {
                    id: p["id"].as_i64().unwrap_or(0),
                    name: p["name"].as_str().unwrap_or("").to_string(),
                    children,
                });
            }
        }
        Ok(parents)
    }

    pub fn client(&self) -> &Client {
        &self.client
    }
} 