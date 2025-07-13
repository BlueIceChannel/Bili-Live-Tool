use api_client::BiliClient;
use anyhow::Result;
use domain::{LoginState, LiveRoomBrief, UserInfo, AreaParent};
use eframe::{egui, App as EApp};
use qrcode::QrCode;
use tokio::runtime::Runtime;
use image::io::Reader as ImageReader;
use image::GenericImageView;
use reqwest;
use serde_json;

struct BiliApp {
    client: BiliClient,
    rt: Runtime,
    login_state: LoginState,
    user_info: Option<UserInfo>,
    room_info: Option<LiveRoomBrief>,
    qr_texture: Option<egui::TextureHandle>,
    qr_url: Option<String>,
    avatar_texture: Option<egui::TextureHandle>,
    cover_texture: Option<egui::TextureHandle>,
    area_list: Vec<AreaParent>,
    selected_parent: usize,
    selected_child: usize,
    selected_area_id: Option<i64>,
    push_addr: String,
    push_key: String,
}

impl BiliApp {
    /// 将二维码 URL 转换为 egui 纹理
    fn load_qr_texture(url: &str, ctx: &egui::Context) -> egui::TextureHandle {
        let code = QrCode::new(url.as_bytes()).expect("QR encode failed");
        let image_side = code.width() as usize;
        let mut pixels: Vec<u8> = Vec::with_capacity(image_side * image_side * 4);
        for y in 0..image_side {
            for x in 0..image_side {
                let black = code[(x, y)];
                let val = if black { 0 } else { 255 };
                pixels.extend_from_slice(&[val, val, val, 255]);
            }
        }
        let img = egui::ColorImage::from_rgba_unmultiplied([
            image_side as usize,
            image_side as usize,
        ], &pixels);
        ctx.load_texture("qr", img, Default::default())
    }

    fn bytes_to_texture(bytes: &[u8], ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if let Ok(img) = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().and_then(|r| r.decode()) {
            let size = [img.width() as usize, img.height() as usize];
            let rgba = img.into_rgba8();
            let pixels = rgba.into_raw();
            let img = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
            Some(ctx.load_texture("net_img", img, Default::default()))
        } else { None }
    }

    fn fetch_texture(rt: &Runtime, client: &reqwest::Client, url: &str, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let fut = async {
            let resp = client.get(url).send().await.ok()?;
            let bytes = resp.bytes().await.ok()?;
            Some(bytes.to_vec())
        };
        if let Some(bytes) = rt.block_on(fut)? {
            Self::bytes_to_texture(&bytes, ctx)
        } else { None }
    }
}

impl Default for BiliApp {
    fn default() -> Self {
        let mut client = BiliClient::new();
        let rt = Runtime::new().expect("failed to create tokio runtime");
        // 启动时尝试刷新 Cookie
        if let Err(e) = rt.block_on(client.refresh_cookies_if_needed()) {
            eprintln!("cookie refresh error: {}", e);
        }

        Self {
            client,
            rt,
            login_state: LoginState::NeedQrCode,
            user_info: None,
            room_info: None,
            qr_texture: None,
            qr_url: None,
            avatar_texture: None,
            cover_texture: None,
            area_list: Vec::new(),
            selected_parent: 0,
            selected_child: 0,
            selected_area_id: None,
            push_addr: String::new(),
            push_key: String::new(),
        }
    }
}

impl eframe::App for BiliApp {
    fn name(&self) -> &str {
        "Bili Live Tool"
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.login_state {
                LoginState::LoggedIn => {
                    if self.user_info.is_none() {
                        // 获取当前登录用户 mid
                        if let Ok(nav_json) = self.rt.block_on(async {
                            self.client
                                .http_client()
                                .get("https://api.bilibili.com/x/web-interface/nav")
                                .send()
                                .await?
                                .json::<serde_json::Value>()
                                .await
                        }) {
                            let mid = nav_json["data"]["mid"].as_u64().unwrap_or(0);
                            if mid != 0 {
                                if let Ok(info) = self.rt.block_on(self.client.get_user_info(mid)) {
                                    self.avatar_texture = Self::fetch_texture(&self.rt, self.client.http_client(), &info.face, ctx);
                                    if info.live_room.room_status == 1 {
                                        self.cover_texture = Self::fetch_texture(&self.rt, self.client.http_client(), &info.live_room.cover, ctx);
                                    }
                                    self.room_info = Some(info.live_room.clone());
                                    self.user_info = Some(info);
                                    if let Ok(list) = self.rt.block_on(self.client.get_area_list()) {
                                        self.area_list = list;
                                    }
                                }
                            }
                        }
                    }

                    if let Some(user) = &self.user_info {
                        if let Some(av) = &self.avatar_texture {
                            ui.image(av, av.size_vec2());
                        }
                        ui.label(format!("昵称: {}", user.name));
                        if user.live_room.room_status == 0 {
                            ui.colored_label(egui::Color32::YELLOW, "该用户无房间");
                            return;
                        }
                        if let Some(room) = &mut self.room_info {
                            ui.horizontal(|h| {
                                h.label("标题: ");
                                h.text_edit_singleline(&mut room.title);
                            });
                            ui.label(format!("直播间号: {}", room.room_id));
                            ui.label(format!("直播状态: {}", if room.live_status == 1 { "直播中" } else { "未开播" }));
                            if let Some(cv) = &self.cover_texture {
                                ui.image(cv, cv.size_vec2());
                            }
                            if ui.button(if room.live_status == 1 { "停止直播" } else { "开始直播" }).clicked() {
                                if room.live_status == 1 {
                                    // stop live
                                    match self.rt.block_on(self.client.stop_live(room.room_id)) {
                                        Ok(()) => {
                                            room.live_status = 0;
                                            self.push_addr.clear();
                                            self.push_key.clear();
                                        }
                                        Err(e) => {
                                            ui.colored_label(egui::Color32::RED, format!("关播失败: {}", e));
                                        }
                                    }
                                } else {
                                    if let Some(area_id) = self.selected_area_id {
                                        match self.rt.block_on(self.client.start_live(room.room_id, area_id)) {
                                            Ok((addr, key)) => {
                                                room.live_status = 1;
                                                self.push_addr = addr;
                                                self.push_key = key;
                                            }
                                            Err(e) => {
                                                ui.colored_label(egui::Color32::RED, format!("开播失败: {}", e));
                                            }
                                        }
                                    } else {
                                        ui.colored_label(egui::Color32::YELLOW, "请先选择分区");
                                    }
                                }
                            }
                            if room.live_status == 1 && !self.push_addr.is_empty() {
                                ui.separator();
                                ui.label("推流地址:");
                                ui.horizontal(|h| {
                                    h.text_edit_singleline(&mut self.push_addr).desired_width(300.0);
                                    if h.button("复制").clicked() {
                                        ctx.output_mut(|o| o.copied_text = self.push_addr.clone());
                                    }
                                });
                                ui.label("推流密钥:");
                                ui.horizontal(|h| {
                                    h.text_edit_singleline(&mut self.push_key).desired_width(300.0);
                                    if h.button("复制").clicked() {
                                        ctx.output_mut(|o| o.copied_text = self.push_key.clone());
                                    }
                                });
                            }
                            if !self.area_list.is_empty() {
                                ui.horizontal(|h| {
                                    // parent combo
                                    let parent_names: Vec<_> = self.area_list.iter().map(|p| p.name.as_str()).collect();
                                    egui::ComboBox::from_label("父分区")
                                        .selected_text(parent_names[self.selected_parent])
                                        .show_ui(h, |ui_inner| {
                                            for (idx, p) in parent_names.iter().enumerate() {
                                                ui_inner.selectable_value(&mut self.selected_parent, idx, *p);
                                            }
                                        });
                                    // ensure selected_child within bounds
                                    if self.selected_parent >= self.area_list.len() { self.selected_parent = 0; }
                                    let child_list = &self.area_list[self.selected_parent].children;
                                    if child_list.is_empty() { return; }
                                    if self.selected_child >= child_list.len() { self.selected_child = 0; }
                                    let child_names: Vec<_> = child_list.iter().map(|c| c.name.as_str()).collect();
                                    egui::ComboBox::from_label("子分区")
                                        .selected_text(child_names[self.selected_child])
                                        .show_ui(h, |ui_inner| {
                                            for (idx, c) in child_names.iter().enumerate() {
                                                ui_inner.selectable_value(&mut self.selected_child, idx, *c);
                                            }
                                        });
                                    self.selected_area_id = Some(child_list[self.selected_child].id);
                                });
                            }
                            if ui.button("保存设置").clicked() {
                                let area_id_opt = self.selected_area_id;
                                let title_clone = room.title.clone();
                                let res = self.rt.block_on(self.client.update_room_info(room.room_id, Some(&title_clone), area_id_opt));
                                match res {
                                    Ok(Some(audit)) => {
                                        if audit.audit_title_status != 0 {
                                            ui.colored_label(egui::Color32::YELLOW, format!("标题审核状态: {} - {}", audit.audit_title_status, audit.audit_title_reason));
                                        } else {
                                            ui.label("更新成功");
                                        }
                                    }
                                    Ok(None) => { ui.label("更新成功"); }
                                    Err(e) => { ui.colored_label(egui::Color32::RED, format!("更新失败: {}", e)); }
                                }
                            }
                        }
                    }
                }
                LoginState::NeedQrCode => {
                    ui.heading("请扫码登录");
                    if self.qr_texture.is_none() {
                        // 首次进入，获取二维码
                        if let Ok(qr) = self.rt.block_on(self.client.fetch_qr_code()) {
                            self.qr_texture = Some(Self::load_qr_texture(&qr.url, ctx));
                            self.qr_url = Some(qr.url);
                        }
                    }
                    if let Some(tex) = &self.qr_texture {
                        let size = tex.size_vec2();
                        ui.image(tex, size);
                    }
                    if ui.button("检查扫码状态").clicked() {
                        if let Some(url) = &self.qr_url {
                            let qr_data = domain::QrCodeData { url: url.clone() };
                            match self.rt.block_on(self.client.poll_qr_login(&qr_data)) {
                                Ok(LoginState::LoggedIn) => {
                                    self.login_state = LoginState::LoggedIn;
                                    self.qr_texture = None;
                                }
                                Ok(LoginState::NeedQrCode) => {
                                    ui.label("尚未扫码或已过期，请稍后重试/刷新。");
                                }
                                Err(e) => {
                                    ui.label(format!("登录失败: {}", e));
                                }
                            }
                        }
                    }
                }
            }
        });
    }
}

// helper methods for BiliClient exposure
trait HttpClientAccessor {
    fn http_client(&self) -> &reqwest::Client;
}

impl HttpClientAccessor for BiliClient {
    fn http_client(&self) -> &reqwest::Client {
        &self.client
    }
    // we also implement client() used above
}

fn main() -> Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Bili Live Tool",
        native_options,
        Box::new(|_cc| Box::new(BiliApp::default())),
    );
    Ok(())
} 