use anyhow::Result;
use clap::{Parser, Subcommand};
use api_client::BiliClient;

#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 检查登录状态
    CheckLogin,
    /// 启动直播
    Start,
    /// 停止直播
    Stop,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = BiliClient::new();

    match cli.command {
        Commands::CheckLogin => {
            let state = client.check_login_state().await?;
            println!("当前登录状态: {:?}", state);
        }
        Commands::Start => {
            let (url, key) = client.start_live().await?;
            println!("推流地址: {}\n推流密钥: {}", url, key);
        }
        Commands::Stop => {
            client.stop_live().await?;
            println!("已发送停播请求");
        }
    }
    Ok(())
} 