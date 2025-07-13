# Bili Live Tool

一个跨平台、低占用、单文件发行的 B 站直播辅助工具，使用 Rust 编写。

## 功能特性

1. **扫码登录**：采用 TV 端二维码登录，自动持久化 Cookie / token，并内置定时刷新逻辑。
2. **直播间信息管理**
   - 修改直播标题
   - 选择直播分区（父 / 子两级级联）
3. **一键开播 / 关播**
   - 获取并显示 RTMP 推流地址 & 密钥
   - 支持一键复制
4. **随机 UA + 自动重试**：请求失败或被风控时自动更换 User-Agent 并重试。
5. **本地缓存**：配置与鉴权信息保存到平台配置目录，如 Windows 的 `%APPDATA%\Bili\LiveTool\auth.json`。
6. **跨平台 GUI**：基于 `eframe/egui`，原生渲染，无第三方运行时。

## 目录结构

```
├── Cargo.toml          # workspace 定义
├── api_client/         # 与 B 站接口交互的核心库
├── domain/             # 通用数据结构
├── gui/                # 桌面 GUI （eframe/egui）
└── cli/                # 可选命令行调试工具
```

## 环境要求

- Rust 1.70+ （建议使用最新 stable）
- Windows / macOS / Linux，需支持 Vulkan / Metal / D3D12/OpenGL 的显卡驱动

首次安装 Rust：
```bash
curl https://sh.rustup.rs -sSf | sh     # macOS / Linux
# Windows 请使用 rustup-init.exe 安装
```

## 编译与运行

### GUI 直接运行
```bash
# 克隆或进入项目根目录
cargo run -p gui            # Debug 构建
# 或
cargo run -p gui --release  # Release 构建，体积更小
```
构建完成后可在 `target/release/` 目录中找到单独可执行文件：

```
# Windows
./target/release/gui.exe
# macOS / Linux
./target/release/gui
```

### CLI 调试工具
```bash
cargo run -p cli -- --help
```

## 使用流程

1. **启动程序**：若存在有效 Cookie，将自动进入主界面；否则生成二维码等待扫码。
2. **扫码登录**：使用 B 站 App 扫码 → 点击"检查扫码状态"。
3. **填写直播信息**：
   - 选择父分区 / 子分区
   - 编辑直播标题
   - 点击"保存设置"
4. **开播 / 关播**：
   - 点击"开始直播"获取推流地址 & 密钥，可复制到推流软件。
   - 推流结束后点击"停止直播"。

## 打包与分发

```
# Windows Release + UPX 压缩示例
cargo build -p gui --release
strip target/release/gui.exe
upx --best target/release/gui.exe
```

macOS 与 Linux 同理，可通过 `strip` / `upx` 或 `cargo lipo`、`AppImage` 等方式封装。

## TODO

- 录播、轮播支持
- 订阅直播状态回调
- 自动更新检查
- 国际化（i18n）

## 致谢

- **接口文档**：本项目所有 B 站接口均参考并遵循
  [SocialSisterYi/bilibili-API-collect](https://github.com/SocialSisterYi/bilibili-API-collect)
  公开整理的 API 说明，特此感谢！
- **生成方式**：本应用核心代码与文档主要由 AI（ChatGPT）在 Pair-Programming 环境中自动生成，人类开发者后期进行测试与微调。

## License

MIT 