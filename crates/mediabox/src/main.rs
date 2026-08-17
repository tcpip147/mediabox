use anyhow::Result;
use rtsp_client::RtspClient;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let mut client = RtspClient::connect("rtsp://210.99.70.120:1935/live/cctv001.stream").await?;
    client.play().await?;

    Ok(())
}
