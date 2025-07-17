#![windows_subsystem = "windows"] // 在Windows上隐藏控制台窗口
use api_client::BiliClient;
use anyhow::Result;
use domain::{LoginState, LiveRoomBrief, UserInfo, AreaParent, WebQrInfo};
use eframe::{egui, Frame};
use qrcode::QrCode;
use tokio::runtime::Runtime;
use image::io::Reader as ImageReader;
use qrcode::Color;
use reqwest;
use std::time::{Instant, Duration};
use std::sync::Arc;

struct BiliApp {
    client: BiliClient,
    rt: Runtime,
    login_state: LoginState,
    user_info: Option<UserInfo>,
    room_info: Option<LiveRoomBrief>,
    qr_texture: Option<egui::TextureHandle>,
    qr_info: Option<WebQrInfo>,
    avatar_texture: Option<egui::TextureHandle>,
    cover_texture: Option<egui::TextureHandle>,
    area_list: Vec<AreaParent>,
    selected_parent: usize,
    selected_child: usize,
    selected_area_id: Option<i64>,
    push_addr: String,
    push_key: String,
    last_qr_poll: Option<Instant>,
    last_user_info_fetch: Option<Instant>,
    area_list_fetch_error: Option<String>,
    version: String,
}

impl BiliApp {
    /// 生成带静区且放大后的二维码纹理
    fn load_qr_texture(url: &str, ctx: &egui::Context) -> egui::TextureHandle {
        let code = QrCode::new(url.as_bytes()).expect("QR encode failed");
        let module_count = code.width() as usize;
        let margin_modules = 4; // 留白
        let scale = 6; // 单模块像素数，控制大小与清晰度
        let img_side = (module_count + margin_modules * 2) * scale;
        let mut pixels = vec![255u8; img_side * img_side * 4]; // white background

        for y in 0..module_count {
            for x in 0..module_count {
                if code[(x, y)] == Color::Dark {
                    let start_x = (x + margin_modules) * scale;
                    let start_y = (y + margin_modules) * scale;
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let idx = ((start_y + dy) * img_side + (start_x + dx)) * 4;
                            pixels[idx..idx + 4].copy_from_slice(&[0, 0, 0, 255]);
                        }
                    }
                }
            }
        }

        let img = egui::ColorImage::from_rgba_unmultiplied([img_side, img_side], &pixels);
        ctx.load_texture("qr", img, Default::default())
    }

    fn bytes_to_texture(bytes: &[u8], ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if let Ok(img) = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().unwrap().decode() {
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
        if let Some(bytes_vec) = rt.block_on(fut) {
            let bytes = &bytes_vec;
            Self::bytes_to_texture(bytes, ctx)
        } else { None }
    }
}

impl Default for BiliApp {
    fn default() -> Self {
        let client = BiliClient::new();
        let rt = Runtime::new().expect("failed to create tokio runtime");
        
        let initial_state = rt.block_on(client.check_login_state()).unwrap_or(LoginState::NeedQrCode);
        
        Self {
            client,
            rt,
            login_state: initial_state,
            user_info: None,
            room_info: None,
            qr_texture: None,
            qr_info: None,
            avatar_texture: None,
            cover_texture: None,
            area_list: Vec::new(),
            selected_parent: 0,
            selected_child: 0,
            selected_area_id: None,
            push_addr: String::new(),
            push_key: String::new(),
            last_qr_poll: None,
            last_user_info_fetch: None,
            area_list_fetch_error: None,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

impl eframe::App for BiliApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().inner_margin(egui::Margin::ZERO))
            .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let frame = egui::Frame::default().inner_margin(egui::Margin::same(16.0));
                frame.show(ui, |ui|{
                    ui.heading("B站直播工具");
                    ui.add_space(10.0);
                    
                    ui.label(format!("当前登录状态: {:?}", self.login_state));
                    ui.add_space(5.0);
                    
                    match self.login_state {
                        LoginState::LoggedIn => {
                            if self.user_info.is_none() {
                                let should_fetch = self.last_user_info_fetch.map_or(true, |t| t.elapsed() >= Duration::from_secs(5));

                                if should_fetch {
                                    self.last_user_info_fetch = Some(Instant::now());
                                    ui.label("正在获取用户信息...");
                                    ctx.request_repaint();
                                    
                                    match self.rt.block_on(self.client.get_self_info()) {
                                        Ok(info) => {
                                            println!("获取到用户详细信息: {:?}", info);
                                            self.avatar_texture = Self::fetch_texture(&self.rt, self.client.client(), &info.face, ctx);
                                            if info.live_room.room_status == 1 {
                                                self.cover_texture = Self::fetch_texture(&self.rt, self.client.client(), &info.live_room.cover, ctx);
                                            }
                                            self.room_info = Some(info.live_room.clone());
                                            self.user_info = Some(info);
                                            if let Ok(list) = self.rt.block_on(self.client.get_area_list()) {
                                                println!("获取到分区列表，数量: {}", list.len());
                                                self.area_list = list;
                                                self.area_list_fetch_error = None;
                                            } else {
                                                let err_msg = "获取分区列表失败，请稍后重试".to_string();
                                                println!("{}", err_msg);
                                                self.area_list_fetch_error = Some(err_msg);
                                            }
                                            // 强制重绘
                                            ctx.request_repaint();
                                        },
                                        Err(e) => {
                                            println!("获取用户信息失败: {}", e);
                                            // 不要立即重置登录状态，让它在5秒后重试
                                        }
                                    }
                                } else {
                                    ui.label("获取用户信息失败，正在重试...");
                                }
                            }

                            if let Some(user) = &self.user_info {
                                ui.horizontal(|ui| {
                                    if let Some(av) = &self.avatar_texture {
                                        let avatar_size = 80.0;
                                        ui.image((av.id(), egui::vec2(avatar_size, avatar_size)));
                                        ui.add_space(10.0);
                                    }
                                    ui.vertical(|ui| {
                                        ui.heading(&user.name);
                                        ui.label(format!("UID: {}", user.mid));
                                    });
                                });
                                ui.add_space(10.0);
                                
                                if user.live_room.room_status == 0 {
                                    let elapsed = self.last_user_info_fetch.map_or(Duration::from_secs(5), |t| t.elapsed());

                                    if elapsed >= Duration::from_secs(5) {
                                        self.user_info = None;
                                        self.room_info = None;
                                        self.last_user_info_fetch = None;
                                        ctx.request_repaint();
                                    } else {
                                        let remaining = Duration::from_secs(5) - elapsed;
                                        ui.colored_label(egui::Color32::YELLOW, format!("未能获取直播间信息，{:.0}秒后自动重试...", remaining.as_secs_f32().ceil()));
                                        ctx.request_repaint_after(Duration::from_secs(1));
                                    }
                                    return;
                                }
                                
                                if let Some(room) = &mut self.room_info {
                                    ui.group(|ui| {
                                        ui.heading("直播间信息");
                                        ui.add_space(5.0);
                                        
                                        ui.horizontal(|ui| {
                                            ui.label("标题: ");
                                            ui.add(egui::TextEdit::singleline(&mut room.title).desired_width(f32::INFINITY));
                                        });
                                        
                                        ui.label(format!("直播间号: {}", room.room_id));
                                        ui.label(format!("直播状态: {}", if room.live_status == 1 { "直播中" } else { "未开播" }));
                                        
                                        if let Some(cv) = &self.cover_texture {
                                            let cover_height = 180.0;
                                            let cover_width = cover_height * 16.0 / 9.0; // 16:9 比例
                                            ui.image((cv.id(), egui::vec2(cover_width, cover_height)));
                                        }
                                        
                                        ui.add_space(10.0);
                                        let area_fetch_failed = self.area_list_fetch_error.is_some();
                                        ui.add_enabled_ui(!area_fetch_failed, |ui| {
                                            if ui.add_sized([200.0, 30.0], egui::Button::new(
                                                if room.live_status == 1 { "停止直播" } else { "开始直播" }
                                            )).clicked() {
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
                                        });
                                        if area_fetch_failed {
                                            ui.colored_label(egui::Color32::RED, self.area_list_fetch_error.as_deref().unwrap_or(""));
                                        }
                                    });
                                    
                                    ui.add_space(10.0);
                                    
                                    if room.live_status == 1 && !self.push_addr.is_empty() {
                                        ui.group(|ui| {
                                            ui.heading("推流信息");
                                            ui.add_space(5.0);
                                            
                                            ui.label("推流地址:");
                                            ui.horizontal(|ui| {
                                                ui.add(egui::TextEdit::singleline(&mut self.push_addr).desired_width(f32::INFINITY));
                                                if ui.button("复制").clicked() {
                                                    ctx.output_mut(|o| o.copied_text = self.push_addr.clone());
                                                }
                                            });
                                            
                                            ui.label("推流密钥:");
                                            ui.horizontal(|ui| {
                                                ui.add(egui::TextEdit::singleline(&mut self.push_key).desired_width(f32::INFINITY));
                                                if ui.button("复制").clicked() {
                                                    ctx.output_mut(|o| o.copied_text = self.push_key.clone());
                                                }
                                            });
                                        });
                                        ui.add_space(10.0);
                                    }
                                    
                                    if !self.area_list.is_empty() {
                                        ui.group(|ui| {
                                            ui.heading("分区设置");
                                            ui.add_space(5.0);
                                            
                                            ui.horizontal(|ui| {
                                                // parent combo
                                                let parent_names: Vec<_> = self.area_list.iter().map(|p| p.name.as_str()).collect();
                                                egui::ComboBox::from_label("父分区")
                                                    .width(200.0)
                                                    .selected_text(parent_names[self.selected_parent])
                                                    .show_ui(ui, |ui| {
                                                        for (idx, p) in parent_names.iter().enumerate() {
                                                            ui.selectable_value(&mut self.selected_parent, idx, *p);
                                                        }
                                                    });
                                                    
                                                ui.add_space(20.0);
                                                
                                                // ensure selected_child within bounds
                                                if self.selected_parent >= self.area_list.len() { self.selected_parent = 0; }
                                                let child_list = &self.area_list[self.selected_parent].children;
                                                if child_list.is_empty() { return; }
                                                if self.selected_child >= child_list.len() { self.selected_child = 0; }
                                                let child_names: Vec<_> = child_list.iter().map(|c| c.name.as_str()).collect();
                                                egui::ComboBox::from_label("子分区")
                                                    .width(200.0)
                                                    .selected_text(child_names[self.selected_child])
                                                    .show_ui(ui, |ui| {
                                                        for (idx, c) in child_names.iter().enumerate() {
                                                            ui.selectable_value(&mut self.selected_child, idx, *c);
                                                        }
                                                    });
                                                self.selected_area_id = Some(child_list[self.selected_child].id);
                                            });
                                        });
                                        ui.add_space(10.0);
                                    }
                                    
                                    let area_fetch_failed = self.area_list_fetch_error.is_some();
                                    ui.add_enabled_ui(!area_fetch_failed, |ui|{
                                        if ui.add_sized([200.0, 30.0], egui::Button::new("保存设置")).clicked() {
                                            let area_id_opt = self.selected_area_id;
                                            let title_clone = room.title.clone();
                                            let res = self.rt.block_on(self.client.update_room_info(room.room_id, Some(&title_clone), area_id_opt));
                                            match res {
                                                Ok(Some(audit)) => {
                                                    if audit.audit_title_status != 0 {
                                                        ui.colored_label(egui::Color32::YELLOW, format!("标题审核状态: {} - {}", audit.audit_title_status, audit.audit_title_reason));
                                                    } else {
                                                        ui.colored_label(egui::Color32::GREEN, "更新成功");
                                                    }
                                                }
                                                Ok(None) => { ui.colored_label(egui::Color32::GREEN, "更新成功"); }
                                                Err(e) => { ui.colored_label(egui::Color32::RED, format!("更新失败: {}", e)); }
                                            }
                                        }
                                    });
                                    if area_fetch_failed {
                                        ui.colored_label(egui::Color32::RED, self.area_list_fetch_error.as_deref().unwrap_or(""));
                                    }
                                }
                            }
                        }
                        LoginState::NeedQrCode => {
                            // 自动轮询扫码结果：每 2 秒检查一次
                            if let Some(qr) = &self.qr_info {
                                let should_poll = self.last_qr_poll.map_or(true, |t| t.elapsed() >= Duration::from_secs(2));
                                if should_poll {
                                    self.last_qr_poll = Some(Instant::now());
                                    if let Ok(LoginState::LoggedIn) = self.rt.block_on(self.client.poll_qr_login(qr)) {
                                        self.login_state = LoginState::LoggedIn;
                                        self.qr_texture = None;
                                        self.qr_info = None;
                                        ctx.request_repaint();
                                        println!("登录成功，状态已更新为LoggedIn");
                                    }
                                }
                            }

                            ui.vertical_centered(|ui| {
                                ui.heading("请扫码登录");
                                ui.add_space(20.0);
                                
                                if self.qr_texture.is_none() {
                                    // 首次进入，获取二维码
                                    if let Ok(qr) = self.rt.block_on(self.client.fetch_qr_code()) {
                                        self.qr_texture = Some(Self::load_qr_texture(&qr.url, ctx));
                                        self.qr_info = Some(qr);
                                    }
                                }
                                
                                if let Some(tex) = &self.qr_texture {
                                    ui.add_space(10.0);
                                    ui.image((tex.id(), tex.size_vec2()));
                                    ui.add_space(20.0);
                                }
                                
                                if ui.add_sized([200.0, 30.0], egui::Button::new("手动检查扫码状态")).clicked() {
                                    if let Some(qr) = &self.qr_info {
                                        match self.rt.block_on(self.client.poll_qr_login(qr)) {
                                            Ok(LoginState::LoggedIn) => {
                                                self.login_state = LoginState::LoggedIn;
                                                self.qr_texture = None;
                                                self.qr_info = None;
                                                ctx.request_repaint();
                                                println!("手动检查：登录成功，状态已更新为LoggedIn");
                                            }
                                            Ok(LoginState::NeedQrCode) => {
                                                ui.colored_label(egui::Color32::YELLOW, "尚未扫码或已过期，请稍后重试/刷新。");
                                            }
                                            Err(e) => {
                                                ui.colored_label(egui::Color32::RED, format!("登录失败: {}", e));
                                            }
                                        }
                                    }
                                }
                            });
                        }
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format!("v{}", self.version));
                        ui.add_space(10.0);
                        ui.hyperlink_to("源代码", "https://github.com/BlueIceChannel/Bili-Live-Tool");
                    });
                });
            });
        });
    }
}

fn load_icon() -> egui::viewport::IconData {
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::load_from_memory(include_bytes!("../assets/icon.png"))
            .expect("Failed to open icon path")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };

    egui::viewport::IconData {
        rgba: icon_rgba,
        width: icon_width,
        height: icon_height,
    }
}

fn main() -> Result<()> {
    let mut native_options = eframe::NativeOptions::default();
    native_options.viewport.inner_size = Some(egui::vec2(800.0, 600.0));
    native_options.viewport.icon = Some(Arc::new(load_icon()));
    
    // 使用默认渲染器
    // native_options.renderer = eframe::Renderer::Glow;
    
    // 启用深色模式
    native_options.follow_system_theme = false;
    native_options.default_theme = eframe::Theme::Dark;
    
    let result = eframe::run_native(
        "Bili Live Tool",
        native_options,
        Box::new(|cc| {
            // --- START NEW LOGIC ---
            // 1. Load font
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "msyh".to_owned(),
                egui::FontData::from_static(include_bytes!("../assets/msyh.ttc")),
            );
            fonts.families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "msyh".to_owned());
            fonts.families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("msyh".to_owned());
            cc.egui_ctx.set_fonts(fonts);

            // 2. Set style
            let mut style = (*cc.egui_ctx.style()).clone();
            style.text_styles = [
                (egui::TextStyle::Heading, egui::FontId::proportional(22.0)),
                (egui::TextStyle::Body, egui::FontId::proportional(16.0)),
                (egui::TextStyle::Monospace, egui::FontId::monospace(14.0)),
                (egui::TextStyle::Button, egui::FontId::proportional(15.0)),
                (egui::TextStyle::Small, egui::FontId::proportional(12.0)),
            ].into();
            
            // Use the dark visuals from egui as a base
            let mut visuals = egui::Visuals::dark();
            visuals.override_text_color = Some(egui::Color32::from_rgb(255, 255, 255));
            
            // Customize widget colors
            visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(30, 30, 30);
            visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::from_rgb(255, 255, 255);
            visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(50, 50, 50);
            visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(255, 255, 255);
            visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(70, 70, 70);
            visuals.widgets.hovered.fg_stroke.color = egui::Color32::from_rgb(255, 255, 255);
            visuals.widgets.active.bg_fill = egui::Color32::from_rgb(90, 90, 90);
            visuals.widgets.active.fg_stroke.color = egui::Color32::from_rgb(255, 255, 255);
            
            visuals.window_fill = egui::Color32::from_rgb(20, 20, 20);
            
            style.visuals = visuals; // Set the customized visuals to the style
            cc.egui_ctx.set_style(style); // Set the full style
            
            Box::new(BiliApp::default())
            // --- END NEW LOGIC ---
        }),
    );
    
    result.map_err(Into::into)
} 