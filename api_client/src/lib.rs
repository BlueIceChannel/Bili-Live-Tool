//! 与 B 站交互的 HTTP 客户端，占位实现。

use anyhow::Result;
use domain::{LoginState, RoomInfo, TokenInfo, Cookie as CookieInfo, AuthData, AreaParent, AreaChild, AuditInfo, UserInfo, LiveRoomBrief, WebQrInfo};
use reqwest::Client;
use std::collections::BTreeMap;
use std::time::SystemTime;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use reqwest::cookie::Jar;
use rand::{seq::SliceRandom, thread_rng};
use reqwest::header::USER_AGENT;
use rsa::{pkcs8::DecodePublicKey, RsaPublicKey, Oaep};
use sha2::Sha256;
use hex;
use regex::Regex;
use reqwest::cookie::CookieStore;

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
        // 启动时从文件加载 cookie
        if let Some(auth) = Self::load_auth() {
            if !auth.cookies.is_empty() {
                println!("加载 {} 条cookie", auth.cookies.len());
                for c in &auth.cookies {
                    let cookie_str = format!("{}={}", c.name, c.value);
                    if let Ok(url) = format!("https://{}", c.domain).parse() {
                       jar.add_cookie_str(&cookie_str, &url);
                    }
                }
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
        let check_url = "https://api.bilibili.com/x/web-interface/nav";
        let resp_json: serde_json::Value = self
            .client
            .get(check_url)
            .header(USER_AGENT, Self::random_ua())
            .send()
            .await?
            .json()
            .await?;
        if resp_json["code"].as_i64().unwrap_or(-1) == 0 {
            if resp_json["data"]["isLogin"].as_bool().unwrap_or(false) {
                return Ok(LoginState::LoggedIn);
            }
        }
        Ok(LoginState::NeedQrCode)
    }

    /// 获取登录二维码 (Web)
    pub async fn fetch_qr_code(&self) -> Result<WebQrInfo> {
        let resp = self
            .client
            .get("https://passport.bilibili.com/x/passport-login/web/qrcode/generate")
            .header(USER_AGENT, Self::random_ua())
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        if resp["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("获取二维码失败: {}", resp["message"].as_str().unwrap_or(""));
        }
        let data = &resp["data"];
        Ok(WebQrInfo {
            url: data["url"].as_str().unwrap_or("").to_string(),
            qrcode_key: data["qrcode_key"].as_str().unwrap_or("").to_string(),
        })
    }

    /// 轮询二维码是否扫描完成 (Web)
    pub async fn poll_qr_login(&self, qr_info: &WebQrInfo) -> Result<LoginState> {
        let poll_url = format!("https://passport.bilibili.com/x/passport-login/web/qrcode/poll?qrcode_key={}", qr_info.qrcode_key);
        let resp = self
            .client
            .get(&poll_url)
            .header(USER_AGENT, Self::random_ua())
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let data = &resp["data"];
        let code = data["code"].as_i64().unwrap_or(-1);
        println!("Web登录轮询响应码: {}", code);
        match code {
            0 => { // 扫码成功
                println!("Web登录成功，保存Cookie...");
                // 登录成功后，B站不会在poll接口返回Set-Cookie，而是由客户端再次请求返回的url来设置。
                // reqwest的cookie_provider会自动处理这个过程，我们只需要确保后续的jar是同一个即可。
                // 手动保存最新的cookie到文件
                let cookies = self.build_cookie_list();
                let auth_data = AuthData { token: TokenInfo::default(), cookies };
                Self::save_auth(&auth_data)?;
                println!("Cookie保存完毕");
                Ok(LoginState::LoggedIn)
            }
            86038 => { // 二维码已失效
                println!("二维码已失效");
                Ok(LoginState::NeedQrCode)
            }
            86090 => { // 二维码已扫，待确认
                println!("二维码已扫，待确认");
                Ok(LoginState::NeedQrCode)
            }
            _ => { // 其他状态，视为未登录
                Ok(LoginState::NeedQrCode)
            }
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

    /// 从活动的 cookie jar 中获取指定名称的 cookie 值
    fn get_cookie_value(&self, name: &str) -> Option<String> {
        let url = "https://bilibili.com".parse().ok()?;
        let cookies = self.jar.cookies(&url)?;
        let cookie_str = cookies.to_str().ok()?;
        for part in cookie_str.split(';') {
            let mut kv = part.trim().splitn(2, '=');
            if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                if k == name {
                    return Some(v.to_string());
                }
            }
        }
        None
    }

    fn build_cookie_list(&self) -> Vec<CookieInfo> {
        // 仅简单解析常用 cookie 并存储
        let url = "https://bilibili.com".parse().unwrap();
        if let Some(cookies_jar) = self.jar.cookies(&url) {
            if let Ok(s) = cookies_jar.to_str() {
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
        let public_key = RsaPublicKey::from_public_key_pem(PUB_KEY_PEM)?;
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
        let check_url = "https://passport.bilibili.com/x/passport-login/web/cookie/info";
        let resp_json: serde_json::Value = self
            .client
            .get(check_url)
            .header(USER_AGENT, Self::random_ua())
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
            let now = SystemTime::now();
            let since_the_epoch = now.duration_since(SystemTime::UNIX_EPOCH).expect("Time went backwards");
            since_the_epoch.as_millis() as i64
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

    /// 获取当前登录用户信息（Web端API）
    pub async fn get_self_info(&self) -> Result<UserInfo> {
        println!("开始获取当前登录用户信息 (Web)");
        let nav_resp: serde_json::Value = self
            .client
            .get("https://api.bilibili.com/x/web-interface/nav")
            .header(USER_AGENT, Self::random_ua())
            .send()
            .await?
            .json()
            .await?;
        
        if nav_resp["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("获取用户信息失败: {}", nav_resp["message"].as_str().unwrap_or(""));
        }

        let data = &nav_resp["data"];
        if !data["isLogin"].as_bool().unwrap_or(false) {
            anyhow::bail!("用户未登录");
        }

        let mid = data["mid"].as_u64().unwrap_or(0);
        if mid == 0 {
            anyhow::bail!("无法获取有效的用户ID");
        }
        
        // 从 /nav 获取基本信息
        let mut user_info = UserInfo {
            mid,
            name: data["uname"].as_str().unwrap_or("").to_string(),
            face: data["face"].as_str().unwrap_or("").to_string(),
            live_room: LiveRoomBrief::default(),
        };
        
        // 从 space/acc/info 获取直播间信息
        let space_url = format!("https://api.bilibili.com/x/space/acc/info?mid={}", mid);
        let space_resp: serde_json::Value = self.client.get(&space_url)
            .header(USER_AGENT, Self::random_ua())
            .send().await?.json().await?;
            
        if space_resp["code"].as_i64().unwrap_or(-1) == 0 {
            if let Some(live_room_data) = space_resp["data"]["live_room"].as_object() {
                 user_info.live_room = LiveRoomBrief {
                    room_status: live_room_data["roomStatus"].as_i64().unwrap_or(0) as i32,
                    live_status: live_room_data["liveStatus"].as_i64().unwrap_or(0) as i32,
                    title: live_room_data["title"].as_str().unwrap_or("").to_string(),
                    cover: live_room_data["cover"].as_str().unwrap_or("").to_string(),
                    room_id: live_room_data["roomid"].as_i64().unwrap_or(0),
                };
            }
        } else {
            println!("警告：获取直播间信息失败: {}", space_resp["message"].as_str().unwrap_or("未知错误"));
        }

        println!("用户信息获取完成: {:?}", user_info);
        Ok(user_info)
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